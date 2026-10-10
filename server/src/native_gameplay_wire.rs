//! Compact authority gameplay messages. No UObject, callable name, or client-owned stat crosses this boundary.
use crate::native_wire::{self as w, Directory, EntityRef, InputFrame};
use hsmp_net::net::wire::{Put, Reader};

pub const CAP_NATIVE_GAMEPLAY: u64 = 1 << 28;
pub const CAP_NATIVE_COMPRESSION: u64 = 1 << 29;
pub const MAX_RESULT_BYTES: usize = 21 + w::MAX_ENTITIES * 160;

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
    out.put_u8(1);
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
    if read!(u8) != 1 {
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
        entities.push(State {
            reference,
            request_seq,
            delivery_seq,
            buttons,
            axes,
            position: p[0],
            rotation: p[1],
            velocity: p[2],
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
