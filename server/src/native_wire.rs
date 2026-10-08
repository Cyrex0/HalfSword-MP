//! Additive native authority messages. These are network DTOs, never shared-memory or engine objects.
use hsmp_net::net::wire::{Put, Reader};
use hsmp_ipc::schema::{combat::Vitals, pose::{Root, POSE_FRAME_MAX}};

pub const K_DIRECTORY: u16 = 0x0AC0;
pub const K_WORLD: u16 = 0x0A10;
pub const K_INPUT: u16 = 0x0A80;
pub const MAX_ENTITIES: usize = 32;
pub const MAX_WORLD_BYTES: usize = 17 + MAX_ENTITIES * (16 + std::mem::size_of::<Root>() + std::mem::size_of::<Vitals>() + 2 + POSE_FRAME_MAX);
const _: () = assert!(MAX_WORLD_BYTES + hsmp_ipc::wire::HDR <= hsmp_net::net::frag::MAX_MESSAGE);
pub const MAX_INPUTS: usize = 256;
pub const INPUT_TIMEOUT_MS: u64 = 250;
pub const BUTTONS_ALL: u32 = 0x7F;
pub const RELEASE_ALL: u32 = 1;
pub const HUMAN: u8 = 0;
pub const AI: u8 = 1;
pub const BOOTING: u8 = 0;
pub const READY: u8 = 1;
pub const LIVE: u8 = 2;
pub const VICTORY: u8 = 3;
pub const DEFEAT: u8 = 4;
pub const FAULT: u8 = 5;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct EntityRef { pub epoch: u64, pub id: u32, pub incarnation: u32 }
impl EntityRef {
    pub fn valid(self) -> bool { self.epoch != 0 && self.id != 0 && self.incarnation != 0 }
}
#[derive(Debug, Clone, PartialEq)]
pub struct Entity {
    pub reference: EntityRef, pub owner_peer: u32, pub slot: u16, pub kind: u8,
    pub controller: u8, pub team: u8,
}
#[derive(Debug, Clone, PartialEq)]
pub struct Directory {
    pub epoch: u64, pub seq: u32, pub state: u8, pub arena: String, pub error: String,
    pub entities: Vec<Entity>,
}
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct InputFrame {
    pub reference: EntityRef, pub seq: u32, pub delivery_seq: u32,
    /// Capture time on the welcome's server timeline, filled by the embedded client.
    pub sample_ms: u64,
    pub buttons: u32, pub flags: u32, pub axes: [f32; 8],
}
impl InputFrame {
    pub fn validate(&self) -> Result<(), &'static str> {
        if !self.reference.valid() || self.seq == 0 { return Err("input identity"); }
        if self.buttons & !BUTTONS_ALL != 0 || self.flags & !RELEASE_ALL != 0 { return Err("input flags"); }
        for (i, x) in self.axes.iter().enumerate() {
            let (lo, hi) = match i { 2 | 3 => (-100.0, 100.0), 4 | 5 => (0.0, 1.0), _ => (-1.0, 1.0) };
            if !x.is_finite() || *x < lo || *x > hi { return Err("input axes"); }
        }
        Ok(())
    }
}
#[derive(Debug, Clone, PartialEq)]
pub struct EntitySnapshot {
    pub reference: EntityRef, pub root: Root, pub vitals: Vitals, pub pose: Vec<u8>,
}
#[derive(Debug, Clone, PartialEq)]
pub struct World { pub epoch: u64, pub directory_seq: u32, pub frame_seq: u32, pub entities: Vec<EntitySnapshot> }

fn fail<T>(_: hsmp_net::net::wire::Truncated) -> Result<T, &'static str> { Err("truncated") }
fn put_ref(b: &mut Vec<u8>, r: EntityRef) { b.put_u64(r.epoch); b.put_u32(r.id); b.put_u32(r.incarnation); }
fn get_ref(r: &mut Reader<'_>) -> Result<EntityRef, &'static str> {
    let v = EntityRef { epoch: r.u64().or_else(fail)?, id: r.u32().or_else(fail)?, incarnation: r.u32().or_else(fail)? };
    if !v.valid() { return Err("entity identity"); } Ok(v)
}
fn text_ok(s: &str, max: usize) -> bool { s.len() <= max && !s.contains('\0') }
fn put_text(b: &mut Vec<u8>, s: &str) { b.put_u8(s.len() as u8); b.put(s.as_bytes()); }
fn get_text(r: &mut Reader<'_>, max: usize) -> Result<String, &'static str> {
    let n = r.u8().or_else(fail)? as usize;
    if n > max { return Err("string bound"); }
    let s = std::str::from_utf8(r.bytes(n).or_else(fail)?).map_err(|_| "utf8")?;
    if !text_ok(s, max) { return Err("string"); } Ok(s.to_owned())
}
pub fn validate_directory(d: &Directory) -> Result<(), &'static str> {
    if d.epoch == 0 || d.seq == 0 || d.state > FAULT || !text_ok(&d.arena, 40) || !text_ok(&d.error, 96)
        || d.entities.is_empty() || d.entities.len() > MAX_ENTITIES { return Err("directory"); }
    let mut ids = std::collections::HashSet::new();
    let mut slots = std::collections::HashSet::new();
    let mut owners = std::collections::HashSet::new();
    for e in &d.entities {
        if !e.reference.valid() || e.reference.epoch != d.epoch || !ids.insert(e.reference.id)
            || e.slot as usize >= MAX_ENTITIES || !slots.insert(e.slot) || e.kind > AI || e.team == 0 || e.team > 4
            || (e.kind == AI && (e.owner_peer != 0 || e.controller != 255))
            || (e.kind == HUMAN && e.controller > 1)
            || (e.owner_peer != 0 && !owners.insert(e.owner_peer)) { return Err("directory entity"); }
    } Ok(())
}
pub fn encode_directory(d: &Directory) -> Result<Vec<u8>, &'static str> {
    validate_directory(d)?;
    let mut b = Vec::with_capacity(64 + d.entities.len() * 26);
    b.put_u64(d.epoch); b.put_u32(d.seq); b.put_u8(d.state); b.put_u8(d.entities.len() as u8);
    put_text(&mut b, &d.arena); put_text(&mut b, &d.error);
    for e in &d.entities { put_ref(&mut b, e.reference); b.put_u32(e.owner_peer); b.put_u16(e.slot); b.put_u8(e.kind); b.put_u8(e.controller); b.put_u8(e.team); }
    Ok(b)
}
pub fn decode_directory(b: &[u8]) -> Result<Directory, &'static str> {
    let mut r = Reader::new(b);
    let epoch = r.u64().or_else(fail)?; let seq = r.u32().or_else(fail)?;
    let state = r.u8().or_else(fail)?; let n = r.u8().or_else(fail)? as usize;
    if n > MAX_ENTITIES { return Err("entity bound"); }
    let arena = get_text(&mut r, 40)?; let error = get_text(&mut r, 96)?;
    let mut entities = Vec::with_capacity(n);
    for _ in 0..n { entities.push(Entity { reference: get_ref(&mut r)?, owner_peer: r.u32().or_else(fail)?, slot: r.u16().or_else(fail)?, kind: r.u8().or_else(fail)?, controller: r.u8().or_else(fail)?, team: r.u8().or_else(fail)? }); }
    if !r.is_empty() { return Err("trailing bytes"); }
    let d = Directory { epoch, seq, state, arena, error, entities }; validate_directory(&d)?; Ok(d)
}
pub fn encode_input(i: &InputFrame) -> Result<Vec<u8>, &'static str> {
    i.validate()?; let mut b = Vec::with_capacity(64); put_ref(&mut b, i.reference);
    b.put_u32(i.seq); b.put_u32(i.delivery_seq); b.put_u64(i.sample_ms); b.put_u32(i.buttons); b.put_u32(i.flags);
    for x in i.axes { b.put_u32(x.to_bits()); } Ok(b)
}
pub fn decode_input(b: &[u8]) -> Result<InputFrame, &'static str> {
    let mut r = Reader::new(b); let mut i = InputFrame { reference: get_ref(&mut r)?, seq: r.u32().or_else(fail)?, delivery_seq: r.u32().or_else(fail)?, sample_ms: r.u64().or_else(fail)?, buttons: r.u32().or_else(fail)?, flags: r.u32().or_else(fail)?, ..Default::default() };
    for x in &mut i.axes { *x = f32::from_bits(r.u32().or_else(fail)?); }
    if !r.is_empty() { return Err("trailing bytes"); } i.validate()?; Ok(i)
}
pub fn validate_world(w: &World) -> Result<(), &'static str> {
    if w.epoch == 0 || w.directory_seq == 0 || w.frame_seq == 0 || w.entities.is_empty() || w.entities.len() > MAX_ENTITIES { return Err("world"); }
    let context = hsmp_pose::posecodec::v2::Context { match_id: w.epoch, round: 1, life: 1 };
    let mut ids = std::collections::HashSet::new();
    for e in &w.entities {
        if !e.reference.valid() || e.reference.epoch != w.epoch || !ids.insert(e.reference.id) || e.pose.len() > POSE_FRAME_MAX { return Err("snapshot identity"); }
        hsmp_ipc::record::view::<Root>(bytemuck::bytes_of(&e.root)).map_err(|_| "root")?;
        hsmp_ipc::record::view::<Vitals>(bytemuck::bytes_of(&e.vitals)).map_err(|_| "vitals")?;
        if e.root.match_id != w.epoch || e.root.round != 1 || e.root.life != 1 || e.vitals.match_id != w.epoch || e.vitals.round != 1 || e.vitals.life != 1 { return Err("snapshot context"); }
        let p = hsmp_pose::posecodec::v2::decode(&e.pose).ok_or("pose")?;
        if p.context != Some(context) || p.ts.floor() as u32 != e.root.ts { return Err("pose context"); }
    } Ok(())
}
pub fn encode_world(w: &World) -> Result<Vec<u8>, &'static str> {
    validate_world(w)?; let mut b = Vec::with_capacity(16 + w.entities.len() * 1300);
    b.put_u64(w.epoch); b.put_u32(w.directory_seq); b.put_u32(w.frame_seq); b.put_u8(w.entities.len() as u8);
    for e in &w.entities { put_ref(&mut b, e.reference); b.put(bytemuck::bytes_of(&e.root)); b.put(bytemuck::bytes_of(&e.vitals)); b.put_u16(e.pose.len() as u16); b.put(&e.pose); }
    if b.len() + 8 > hsmp_net::net::frag::MAX_MESSAGE { return Err("world bound"); } Ok(b)
}
pub fn decode_world(b: &[u8]) -> Result<World, &'static str> {
    let mut r = Reader::new(b); let epoch = r.u64().or_else(fail)?; let directory_seq = r.u32().or_else(fail)?; let frame_seq = r.u32().or_else(fail)?;
    let n = r.u8().or_else(fail)? as usize; if n > MAX_ENTITIES { return Err("entity bound"); }
    let mut entities = Vec::with_capacity(n);
    for _ in 0..n {
        let reference = get_ref(&mut r)?;
        let root = bytemuck::pod_read_unaligned(r.bytes(std::mem::size_of::<Root>()).or_else(fail)?);
        let vitals = bytemuck::pod_read_unaligned(r.bytes(std::mem::size_of::<Vitals>()).or_else(fail)?);
        let len = r.u16().or_else(fail)? as usize; if len > POSE_FRAME_MAX { return Err("pose bound"); }
        let pose = r.bytes(len).or_else(fail)?.to_vec(); entities.push(EntitySnapshot { reference, root, vitals, pose });
    }
    if !r.is_empty() { return Err("trailing bytes"); }
    let w = World { epoch, directory_seq, frame_seq, entities }; validate_world(&w)?; Ok(w)
}
pub fn matches_directory(w: &World, d: &Directory) -> bool {
    w.epoch == d.epoch && w.directory_seq == d.seq && w.entities.len() == d.entities.len()
        && w.entities.iter().all(|e| d.entities.iter().any(|x| x.reference == e.reference))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_wire_rejects_all_truncations_trailing_bytes_and_unbounded_fields() {
        let reference=EntityRef{epoch:9,id:1,incarnation:2};
        let d=Directory{epoch:9,seq:1,state:READY,arena:"Map_Arena_Yard".into(),error:String::new(),entities:vec![Entity{reference,owner_peer:1,slot:0,kind:HUMAN,controller:0,team:1}]};
        let encoded=encode_directory(&d).unwrap();assert_eq!(decode_directory(&encoded).unwrap(),d);
        for n in 0..encoded.len(){assert!(decode_directory(&encoded[..n]).is_err(),"accepted directory truncation {n}");}
        let mut extra=encoded.clone();extra.push(0);assert!(decode_directory(&extra).is_err());
        let input=InputFrame{reference,seq:1,sample_ms:4,axes:[0.2,0.0,3.0,0.0,1.0,0.0,0.0,0.0],..Default::default()};
        let encoded=encode_input(&input).unwrap();assert_eq!(decode_input(&encoded).unwrap(),input);
        for n in 0..encoded.len(){assert!(decode_input(&encoded[..n]).is_err(),"accepted input truncation {n}");}
        let mut bad=input;bad.axes[2]=f32::NAN;assert!(encode_input(&bad).is_err());bad=input;bad.buttons=0x80;assert!(encode_input(&bad).is_err());
        let mut bad=d.clone();bad.entities[0].slot=32;assert!(encode_directory(&bad).is_err());bad=d.clone();bad.entities.push(bad.entities[0].clone());assert!(encode_directory(&bad).is_err());
        let context=hsmp_pose::posecodec::v2::Context{match_id:9,round:1,life:1};
        let world=World{epoch:9,directory_seq:1,frame_seq:1,entities:vec![EntitySnapshot{reference,
            root:Root{rot:[0.0,0.0,0.0,1.0],match_id:9,round:1,life:1,..Default::default()},
            vitals:Vitals{match_id:9,round:1,life:1,..Default::default()},
            pose:hsmp_pose::posecodec::v2::encode(&hsmp_pose::posecodec::v2::Full{context:Some(context),..Default::default()})}]};
        let encoded=encode_world(&world).unwrap();assert_eq!(decode_world(&encoded).unwrap(),world);
        for n in 0..encoded.len(){assert!(decode_world(&encoded[..n]).is_err(),"accepted world truncation {n}");}
        let mut wrong=world.clone();wrong.entities[0].root.life=2;assert!(encode_world(&wrong).is_err());
        wrong=world.clone();wrong.entities[0].pose.resize(POSE_FRAME_MAX+1,0);assert!(encode_world(&wrong).is_err());
        wrong=world.clone();wrong.entities[0].reference.incarnation+=1;assert!(!matches_directory(&wrong,&d));
        wrong=world;wrong.epoch+=1;assert!(!matches_directory(&wrong,&d));
    }
}
