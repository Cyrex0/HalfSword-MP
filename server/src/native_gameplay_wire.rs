//! Compact authority gameplay messages. No UObject, callable name, or client-owned stat crosses this boundary.
use crate::native_descriptor as d;
use crate::native_wire::{self as w, Directory, EntityRef, InputFrame};
use hsmp_net::net::wire::{Put, Reader};
use serde::{Deserialize, Serialize};

pub const CAP_NATIVE_GAMEPLAY: u64 = 1 << 28;
pub const CAP_NATIVE_COMPRESSION: u64 = 1 << 29;
pub const CAP_NATIVE_GAMEPLAY_QUATERNION: u64 = 1 << 30;
pub const CAP_NATIVE_GAMEPLAY_CACHE: u64 = 1 << 31;
pub const RESULT_VERSION: u8 = 3;
pub const MAX_RESULT_BYTES: usize = 21 + w::MAX_ENTITIES * 216;
pub fn gameplay_capable(caps: u64) -> bool {
    let required = CAP_NATIVE_GAMEPLAY | CAP_NATIVE_GAMEPLAY_QUATERNION | CAP_NATIVE_GAMEPLAY_CACHE;
    caps & required == required
}
pub const RECIPE_SCHEMA: u16 = 1;
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Weapon {
    pub id: u32,
    pub actor_class: String,
    pub passport: d::WeaponPassport,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Equipment {
    pub armor: Vec<d::ArmorInSlot>,
    pub weapons: Vec<Weapon>,
    pub hands: Vec<d::ItemSlot>,
    pub sheaths: Vec<d::WeaponBinding>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recipe {
    pub schema: u16,
    pub actor_class: String,
    pub team: i32,
    pub passport: d::CharacterPassport,
    pub construction: d::Construction,
    pub equipment: Equipment,
}
impl Recipe {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.schema != RECIPE_SCHEMA || !d::asset(&self.actor_class, false) {
            return Err("gameplay recipe schema/class");
        }
        self.passport.validate()?;
        self.construction.validate()?;
        d::armor_map(&self.equipment.armor, true)?;
        let e = &self.equipment;
        if e.weapons.len() > d::MAX_WEAPONS
            || !d::sorted_slots(&e.hands, 2, |r| r.slot)
            || e.sheaths.len() > 5
        {
            return Err("gameplay equipment bound");
        }
        let mut ids = std::collections::HashSet::new();
        for weapon in &e.weapons {
            if weapon.id == 0 || !ids.insert(weapon.id) || !d::asset(&weapon.actor_class, false) {
                return Err("gameplay weapon identity");
            }
            weapon.passport.validate()?;
        }
        if e.hands.iter().any(|r| !ids.contains(&r.item)) {
            return Err("gameplay hand binding");
        }
        let mut sheaths = std::collections::HashSet::new();
        for row in &e.sheaths {
            if !WEAPON_FIELDS[2..].contains(&row.field.as_str())
                || !sheaths.insert(&row.field)
                || !ids.contains(&row.item)
            {
                return Err("gameplay sheath binding");
            }
        }
        Ok(())
    }
}
pub const WEAPON_FIELDS: [&str; 7] = [
    "Weapon R",
    "Weapon L",
    "Weapon Slot R 1",
    "Weapon Slot R 2",
    "Weapon Slot Back",
    "Weapon Slot L 1",
    "Weapon Slot L 2",
];
#[derive(Clone, Debug, PartialEq)]
pub struct Bootstrap {
    pub reference: EntityRef,
    pub slot: u16,
    pub directory_seq: u32,
    pub revision: u32,
    pub source_frame_seq: u32,
    pub recipe: Recipe,
}
pub fn encode_bootstrap(v: &Bootstrap) -> Result<Vec<u8>, &'static str> {
    if !v.reference.valid()
        || v.slot as usize >= w::MAX_ENTITIES
        || v.directory_seq == 0
        || v.revision == 0
        || v.source_frame_seq == 0
    {
        return Err("gameplay bootstrap identity");
    }
    v.recipe.validate()?;
    let body = serde_json::to_vec(&v.recipe).map_err(|_| "gameplay recipe encoding")?;
    if body.len() > d::MAX_RECIPE_BYTES {
        return Err("gameplay recipe bound");
    }
    let mut out = Vec::with_capacity(30 + body.len());
    out.put_u64(v.reference.epoch);
    out.put_u32(v.reference.id);
    out.put_u32(v.reference.incarnation);
    out.put_u16(v.slot);
    out.put_u32(v.directory_seq);
    out.put_u32(v.revision);
    out.put_u32(v.source_frame_seq);
    out.extend(body);
    Ok(out)
}
pub fn decode_bootstrap(bytes: &[u8]) -> Result<Bootstrap, &'static str> {
    if bytes.len() < 30 || bytes.len() > d::MAX_RECIPE_BYTES + 30 {
        return Err("gameplay bootstrap bound");
    }
    let mut r = Reader::new(bytes);
    let reference = EntityRef {
        epoch: r.u64().map_err(|_| "gameplay bootstrap epoch")?,
        id: r.u32().map_err(|_| "gameplay bootstrap id")?,
        incarnation: r.u32().map_err(|_| "gameplay bootstrap incarnation")?,
    };
    let v = Bootstrap {
        reference,
        slot: r.u16().map_err(|_| "gameplay bootstrap slot")?,
        directory_seq: r.u32().map_err(|_| "gameplay bootstrap directory")?,
        revision: r.u32().map_err(|_| "gameplay bootstrap revision")?,
        source_frame_seq: r.u32().map_err(|_| "gameplay bootstrap frame")?,
        recipe: serde_json::from_slice(&bytes[30..]).map_err(|_| "gameplay recipe schema")?,
    };
    encode_bootstrap(&v)?;
    Ok(v)
}
#[cfg(test)]
pub(crate) fn fixture_recipe() -> Recipe {
    let source = d::fixture_recipe();
    Recipe {
        schema: RECIPE_SCHEMA,
        actor_class: source.actor_class,
        team: source.team,
        passport: source.passport,
        construction: source.construction,
        equipment: Equipment {
            armor: source.equipment.armor,
            weapons: source
                .equipment
                .weapons
                .into_iter()
                .map(|w| Weapon {
                    id: w.id,
                    actor_class: w.actor_class,
                    passport: w.passport,
                })
                .collect(),
            hands: source.equipment.hands,
            sheaths: source.equipment.sheaths,
        },
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NativeScalar {
    F32(u32),
    F64(u64),
}
impl NativeScalar {
    pub fn value(self) -> f64 {
        match self {
            Self::F32(v) => f32::from_bits(v) as f64,
            Self::F64(v) => f64::from_bits(v),
        }
    }
    pub fn validate(self) -> Result<(), &'static str> {
        if self.value().is_finite() {
            Ok(())
        } else {
            Err("nonfinite native gameplay stat")
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    pub directory_seq: u32,
    pub input: InputFrame,
}
#[derive(Clone, Debug, PartialEq)]
pub struct State {
    pub reference: EntityRef,
    pub request_seq: u32,
    pub delivery_seq: u32,
    pub buttons: u32,
    pub axes: [f32; 8],
    pub position: [f64; 3],
    pub rotation: [f64; 3],
    pub velocity: [f64; 3],
    /// Exact native quaternion in XYZW order. No Euler reconstruction.
    pub orientation: [f64; 4],
    /// Exact Euler paired with orientation in the original native root cache.
    pub cache_rotation: [f64; 3],
    pub health: NativeScalar,
    pub stamina: NativeScalar,
}
#[derive(Clone, Debug, PartialEq)]
pub struct ResultFrame {
    pub epoch: u64,
    pub directory_seq: u32,
    pub authority_tick: u32,
    pub entities: Vec<State>,
}
impl ResultFrame {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.epoch == 0
            || self.directory_seq == 0
            || self.authority_tick == 0
            || self.entities.is_empty()
            || self.entities.len() > w::MAX_ENTITIES
        {
            return Err("gameplay result context");
        }
        for (i, e) in self.entities.iter().enumerate() {
            if !e.reference.valid()
                || e.reference.epoch != self.epoch
                || self.entities[..i]
                    .iter()
                    .any(|old| old.reference.id == e.reference.id)
            {
                return Err("gameplay result identity");
            }
            if (e.request_seq == 0) != (e.delivery_seq == 0) {
                return Err("gameplay execution acknowledgement");
            }
            InputFrame {
                reference: e.reference,
                seq: e.request_seq.max(1),
                buttons: e.buttons,
                axes: e.axes,
                ..Default::default()
            }
            .validate()?;
            if e.position
                .iter()
                .chain(&e.rotation)
                .chain(&e.velocity)
                .chain(&e.orientation)
                .chain(&e.cache_rotation)
                .any(|v| !v.is_finite())
            {
                return Err("gameplay result transform");
            }
            e.health.validate()?;
            e.stamina.validate()?;
        }
        Ok(())
    }
    pub fn matches(&self, d: &Directory) -> bool {
        self.epoch == d.epoch
            && self.directory_seq == d.seq
            && self.entities.len() == d.entities.len()
            && self
                .entities
                .iter()
                .all(|e| d.entities.iter().any(|row| row.reference == e.reference))
    }
}
pub fn encode_request(v: &Request) -> Result<Vec<u8>, &'static str> {
    if v.directory_seq == 0 || v.input.flags != 0 || v.input.delivery_seq != 0 {
        return Err("gameplay request context");
    }
    let mut out = Vec::new();
    out.put_u32(v.directory_seq);
    out.extend(w::encode_input(&v.input)?);
    Ok(out)
}
pub fn decode_request(bytes: &[u8]) -> Result<Request, &'static str> {
    if bytes.len() < 4 {
        return Err("truncated gameplay request");
    }
    let v = Request {
        directory_seq: u32::from_le_bytes(bytes[..4].try_into().unwrap()),
        input: w::decode_input(&bytes[4..])?,
    };
    encode_request(&v)?;
    Ok(v)
}
fn scalar(out: &mut Vec<u8>, v: NativeScalar) {
    match v {
        NativeScalar::F32(bits) => {
            out.put_u8(1);
            out.put_u32(bits)
        }
        NativeScalar::F64(bits) => {
            out.put_u8(2);
            out.put_u64(bits)
        }
    }
}
fn read_scalar(r: &mut Reader<'_>) -> Result<NativeScalar, &'static str> {
    let v = match r.u8().map_err(|_| "truncated gameplay stat")? {
        1 => NativeScalar::F32(r.u32().map_err(|_| "truncated gameplay stat")?),
        2 => NativeScalar::F64(r.u64().map_err(|_| "truncated gameplay stat")?),
        _ => return Err("native gameplay stat type"),
    };
    v.validate()?;
    Ok(v)
}
pub fn encode_result(v: &ResultFrame) -> Result<Vec<u8>, &'static str> {
    v.validate()?;
    let mut out = Vec::new();
    out.put_u8(RESULT_VERSION);
    out.put_u64(v.epoch);
    out.put_u32(v.directory_seq);
    out.put_u32(v.authority_tick);
    out.put_u32(v.entities.len() as u32);
    for e in &v.entities {
        out.put_u64(e.reference.epoch);
        out.put_u32(e.reference.id);
        out.put_u32(e.reference.incarnation);
        out.put_u32(e.request_seq);
        out.put_u32(e.delivery_seq);
        out.put_u32(e.buttons);
        for a in e.axes {
            out.put_u32(a.to_bits());
        }
        for v in e.position.into_iter().chain(e.rotation).chain(e.velocity) {
            out.put_u64(v.to_bits());
        }
        for v in e.orientation {
            out.put_u64(v.to_bits());
        }
        for v in e.cache_rotation {
            out.put_u64(v.to_bits());
        }
        scalar(&mut out, e.health);
        scalar(&mut out, e.stamina);
    }
    Ok(out)
}
pub fn decode_result(bytes: &[u8]) -> Result<ResultFrame, &'static str> {
    if bytes.len() > MAX_RESULT_BYTES {
        return Err("gameplay result capacity");
    }
    let mut r = Reader::new(bytes);
    macro_rules! read {
        ($m:ident) => {
            r.$m().map_err(|_| "truncated gameplay result")?
        };
    }
    if read!(u8) != RESULT_VERSION {
        return Err("gameplay result version");
    }
    let epoch = read!(u64);
    let directory_seq = read!(u32);
    let authority_tick = read!(u32);
    let count = read!(u32) as usize;
    if count == 0 || count > w::MAX_ENTITIES {
        return Err("gameplay result count");
    }
    let mut entities = Vec::with_capacity(count);
    for _ in 0..count {
        let reference = EntityRef {
            epoch: read!(u64),
            id: read!(u32),
            incarnation: read!(u32),
        };
        let request_seq = read!(u32);
        let delivery_seq = read!(u32);
        let buttons = read!(u32);
        let mut axes = [0.; 8];
        for a in &mut axes {
            *a = f32::from_bits(read!(u32));
        }
        let mut p = [[0.; 3]; 3];
        for v in p.iter_mut().flatten() {
            *v = f64::from_bits(read!(u64));
        }
        let mut orientation = [0.; 4];
        for v in &mut orientation {
            *v = f64::from_bits(read!(u64));
        }
        let mut cache_rotation = [0.; 3];
        for v in &mut cache_rotation {
            *v = f64::from_bits(read!(u64));
        }
        entities.push(State {
            reference,
            request_seq,
            delivery_seq,
            buttons,
            axes,
            position: p[0],
            rotation: p[1],
            velocity: p[2],
            orientation,
            cache_rotation,
            health: read_scalar(&mut r)?,
            stamina: read_scalar(&mut r)?,
        });
    }
    if !r.is_empty() {
        return Err("trailing gameplay result bytes");
    }
    let v = ResultFrame {
        epoch,
        directory_seq,
        authority_tick,
        entities,
    };
    v.validate()?;
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn gameplay_bootstrap_keeps_complete_passports_aliases_and_exact_float_bits() {
        let mut recipe = fixture_recipe();
        recipe.construction.height_rate = 1.0000000000000002;
        recipe.construction.scale_mutation_inhibitor = -0.0;
        recipe.passport.hair_length = 1.0000000000000002;
        // Complete synthetic offline passport; this is codec evidence, not a
        // claim about a native actor or its observed equipment.
        let id = 1;
        let weapon = d::WeaponPassport {
            class: "/Game/Test/Weapon.Weapon_C".into(),
            id: 42,
            name: "Codec weapon".into(),
            head_sub1: "/Game/Test/HeadSub1.HeadSub1".into(),
            head_sub2: String::new(),
            head: "/Game/Test/Head.Head".into(),
            guard: "/Game/Test/Guard.Guard".into(),
            pommel: "/Game/Test/Pommel.Pommel".into(),
            grip: "/Game/Test/Grip.Grip".into(),
            head_size: [1.0000000000000002, -0.0, 3.0],
            guard_size: [4.0, 5.0, 6.0],
            grip_size: [7.0, 8.0, 9.0],
            pommel_size: [10.0, 11.0, 12.0],
            mass_head: 13.0,
            mass_guard: 14.0,
            mass_grip: 15.0,
            mass_pommel: 16.0,
            mat_steel: 17,
            mat_colored: 18,
            mat_wood: 19,
            mat_leather: 20,
            color_wood: [0.1, 0.2, 0.3, 0.4],
            color_leather: [0.5, 0.6, 0.7, 0.8],
            price: 21.000000000000004,
            tier: 22,
        };
        recipe.passport.equipment.hands = vec![d::WeaponInSlot {
            slot: 0,
            passport: weapon.clone(),
        }];
        recipe.equipment.weapons = vec![Weapon {
            id,
            actor_class: weapon.class.clone(),
            passport: weapon,
        }];
        recipe.equipment.hands = vec![
            d::ItemSlot { slot: 0, item: id },
            d::ItemSlot { slot: 1, item: id },
        ];
        recipe.equipment.sheaths = WEAPON_FIELDS[2..]
            .iter()
            .map(|field| d::WeaponBinding {
                field: (*field).into(),
                item: id,
            })
            .collect();
        let bootstrap = Bootstrap {
            reference: EntityRef {
                epoch: 1,
                id: 1,
                incarnation: 2,
            },
            slot: 0,
            directory_seq: 3,
            revision: 4,
            source_frame_seq: 5,
            recipe,
        };
        let bytes = encode_bootstrap(&bootstrap).unwrap();
        let decoded = decode_bootstrap(&bytes).unwrap();
        assert_eq!(decoded, bootstrap);
        assert_eq!(
            decoded.recipe.construction.height_rate.to_bits(),
            bootstrap.recipe.construction.height_rate.to_bits()
        );
        assert_eq!(
            decoded
                .recipe
                .construction
                .scale_mutation_inhibitor
                .to_bits(),
            (-0.0f64).to_bits()
        );
        assert_eq!(
            decoded.recipe.equipment.hands.len() + decoded.recipe.equipment.sheaths.len(),
            7
        );
        assert_eq!(
            decoded.recipe.equipment.weapons[0].passport.head_size[0].to_bits(),
            bootstrap.recipe.equipment.weapons[0].passport.head_size[0].to_bits()
        );
        assert_eq!(
            decoded.recipe.equipment.weapons[0].passport.head_size[1].to_bits(),
            (-0.0f64).to_bits()
        );
        // Absent aliases encode observed native nulls independently of aliases
        // that refer to the same retained weapon dictionary entry.
        let mut nullable = bootstrap.clone();
        nullable.recipe.equipment.hands.remove(1);
        nullable
            .recipe
            .equipment
            .sheaths
            .retain(|r| r.field == "Weapon Slot R 1" || r.field == "Weapon Slot L 2");
        assert_eq!(
            decode_bootstrap(&encode_bootstrap(&nullable).unwrap()).unwrap(),
            nullable
        );
        let original = serde_json::to_value(&bootstrap.recipe).unwrap();
        for field in ["passport", "construction", "equipment"] {
            let mut missing = original.clone();
            missing.as_object_mut().unwrap().remove(field);
            assert!(serde_json::from_value::<Recipe>(missing).is_err());
        }
        let mut unknown = original;
        unknown
            .as_object_mut()
            .unwrap()
            .insert("components".into(), serde_json::json!([]));
        assert!(serde_json::from_value::<Recipe>(unknown).is_err());
        assert!(d::SourceRecipe::decode_recipe(&bytes[30..]).is_err());
    }
    fn frame() -> ResultFrame {
        ResultFrame {
            epoch: 9,
            directory_seq: 3,
            authority_tick: 2,
            entities: vec![State {
                reference: EntityRef {
                    epoch: 9,
                    id: 1,
                    incarnation: 2,
                },
                request_seq: 7,
                delivery_seq: 4,
                buttons: 1,
                axes: [-0.; 8],
                position: [-0., 1.0000000000000002, 1e20],
                rotation: [0.; 3],
                velocity: [0.; 3],
                orientation: [-0., 0.5000000000000001, -0.5, 0.7071067811865475],
                cache_rotation: [-0., 90.00000000000001, 1.0000000000000002],
                health: NativeScalar::F32(99.125f32.to_bits()),
                stamina: NativeScalar::F64(77.12345678901234f64.to_bits()),
            }],
        }
    }
    #[test]
    fn exact_typed_state_and_truncation() {
        let original = frame();
        let bytes = encode_result(&original).unwrap();
        let got = decode_result(&bytes).unwrap();
        assert_eq!(got, original);
        assert_eq!(got.entities[0].position[0].to_bits(), (-0f64).to_bits());
        for (actual, expected) in got.entities[0]
            .orientation
            .into_iter()
            .zip(original.entities[0].orientation)
        {
            assert_eq!(actual.to_bits(), expected.to_bits());
        }
        assert_eq!(bytes[0], RESULT_VERSION);
        for (actual, expected) in got.entities[0]
            .cache_rotation
            .into_iter()
            .zip(original.entities[0].cache_rotation)
        {
            assert_eq!(actual.to_bits(), expected.to_bits());
        }
        let mut no_cache = bytes.clone();
        let cache_offset = 21 + 16 + 12 + 32 + 72 + 32;
        no_cache.drain(cache_offset..cache_offset + 24);
        no_cache[0] = 2;
        assert!(decode_result(&no_cache).is_err());
        no_cache[0] = RESULT_VERSION;
        assert!(decode_result(&no_cache).is_err());
        let mut legacy = bytes.clone();
        legacy[0] = 1;
        assert!(decode_result(&legacy).is_err());
        // Old bodies omit the mandatory four native doubles; neither version
        // label can make those bytes a complete current result.
        let orientation_offset = 21 + 16 + 12 + 32 + 72;
        legacy.drain(orientation_offset..orientation_offset + 32);
        assert!(decode_result(&legacy).is_err());
        legacy[0] = RESULT_VERSION;
        assert!(decode_result(&legacy).is_err());
        for n in 0..bytes.len() {
            assert!(decode_result(&bytes[..n]).is_err());
        }
        let mut more = bytes;
        more.push(0);
        assert!(decode_result(&more).is_err());
    }
    #[test]
    fn duplicate_generation_and_stat_rejections() {
        let mut v = frame();
        v.entities.push(v.entities[0].clone());
        assert!(v.validate().is_err());
        v = frame();
        v.entities[0].health = NativeScalar::F64(f64::NAN.to_bits());
        assert!(v.validate().is_err());
        v = frame();
        v.entities[0].delivery_seq = 0;
        assert!(v.validate().is_err());
        v = frame();
        v.entities[0].orientation[2] = f64::NAN;
        assert!(v.validate().is_err());
        v = frame();
        v.entities[0].cache_rotation[1] = f64::INFINITY;
        assert!(v.validate().is_err());
    }
    #[test]
    fn gameplay_requires_exact_quaternion_capability() {
        assert!(!gameplay_capable(CAP_NATIVE_GAMEPLAY));
        assert!(!gameplay_capable(CAP_NATIVE_GAMEPLAY_QUATERNION));
        assert!(!gameplay_capable(
            CAP_NATIVE_GAMEPLAY | CAP_NATIVE_GAMEPLAY_QUATERNION
        ));
        assert!(gameplay_capable(
            CAP_NATIVE_GAMEPLAY | CAP_NATIVE_GAMEPLAY_QUATERNION | CAP_NATIVE_GAMEPLAY_CACHE
        ));
    }
    #[test]
    fn only_validated_inputs_are_requests() {
        let v = Request {
            directory_seq: 3,
            input: InputFrame {
                reference: frame().entities[0].reference,
                seq: 7,
                buttons: 1,
                ..Default::default()
            },
        };
        assert_eq!(decode_request(&encode_request(&v).unwrap()).unwrap(), v);
        let mut forged = v;
        forged.input.delivery_seq = 1;
        assert!(encode_request(&forged).is_err());
    }
}
