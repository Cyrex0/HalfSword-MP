//! The pose hot paths: `put_root`, `put_weapon`, `put_pose`, `put_lead` (scalar / flat-array
//! arguments). Each builds its ABI 2 record once, here, on the game thread
//! (`hsmp_pose::sample`: rotator -> quaternion, pose codec v2) and publishes it into its slot
//! (`local_root` / `local_weapon` / `local_pose`); the sidecar frames those bytes as they are.

use std::ffi::c_int;

use hsmp_ipc::schema::pose::{PoseBuf, PoseLead, K_POSE, K_ROOT, K_WEAPON};
use hsmp_pose::sample::{self, PoseArgs, BONE_NUMS, CONTROL_NUMS, WEAPON_NUMS};
use hsmp_pose::posecodec::v2::{BodyStriker, MAX_BODY_STRIKERS, WeaponBox, MAX_WEAPON_BOXES};

/// Integer Lua fields preserve all 64 match-id bits (no floating conversion).
pub(crate) unsafe fn read_pose_context(L: *mut lua_State, index: c_int) -> Option<hsmp_pose::posecodec::v2::Context> {
    unsafe {
        if !is_table(L,index) {return None;}
        let t=lua_absindex(L,index);
        let get=|name:&str| {rawget_str(L,t,name);let v=arg_int(L,-1);pop(L,1);v};
        let match_id=get("match_id").unwrap_or(0) as u64;
        let round=get("round").filter(|v|*v>0 && *v<=u32::MAX as i64).unwrap_or(0) as u32;
        let life=get("life").filter(|v|*v>0 && *v<=u16::MAX as i64).unwrap_or(0) as u16;
        Some(hsmp_pose::posecodec::v2::Context{match_id,round,life})
    }
}

pub(crate) unsafe fn read_body_strikers(L: *mut lua_State, index: c_int) -> Option<Vec<BodyStriker>> {
    unsafe {
        if !is_table(L,index) { return None; }
        let t=lua_absindex(L,index); let len=lua_rawlen(L,t) as usize;
        if len%13!=0 || len/13>MAX_BODY_STRIKERS { return Some(Vec::new()); }
        let mut out:Vec<BodyStriker>=Vec::with_capacity(len/13);
        for i in 0..len/13 {
            let mut a=[0.0;13];
            if !geti_nums(L,t,(i*13+1) as i64,&mut a) || a[..3].iter().any(|v|!v.is_finite() || *v!=v.floor()) {return Some(Vec::new());}
            let s=BodyStriker {part:a[0] as u8,component:a[1] as u8,kind:a[2] as u8,p:[a[3] as f32,a[4] as f32,a[5] as f32],
                q:[a[6] as f32,a[7] as f32,a[8] as f32,a[9] as f32],half:[a[10] as f32,a[11] as f32,a[12] as f32]};
            if !s.valid() || out.iter().any(|old|old.part==s.part && old.component==s.component) {return Some(Vec::new());}
            out.push(s);
        }
        Some(out)
    }
}

/// Protocol10 flat geometry: fifteen numbers per row (component, p, q, half,
/// native scale or zero, native cutting-parent ordinal or zero).
/// A malformed optional shape never manufactures geometry.
pub(crate) unsafe fn read_weapon_boxes(L: *mut lua_State, index: c_int) -> Vec<WeaponBox> {
    unsafe {
        if !is_table(L, index) { return Vec::new(); }
        let t = lua_absindex(L, index);
        let len = lua_rawlen(L, t) as usize;
        let stride=15;
        if len % stride != 0 || len / stride > MAX_WEAPON_BOXES { return Vec::new(); }
        rawget_str(L,t,"class_hash");let class_hash=arg_int(L,-1).unwrap_or(0) as u32;pop(L,1);
        let mut boxes = Vec::with_capacity(len / stride);
        for i in 0..len / stride {
            let mut a = [0.0; 11];
            if !geti_nums(L, t, (i * stride + 1) as i64, &mut a) { return Vec::new(); }
            let mut scale=[0.0;3];
            if !geti_nums(L,t,(i*stride+12) as i64,&mut scale) {return Vec::new();}
            let mut parent=[0.0];
            if !geti_nums(L,t,(i*stride+15) as i64,&mut parent) || parent[0]!=parent[0].floor() || !(0.0..=15.0).contains(&parent[0]) {return Vec::new();}
            let native_scale=if scale==[0.0;3] {None} else {Some(scale.map(|v|v as f32))};
            if a[0] != a[0].floor() { return Vec::new(); }
            let b = WeaponBox { component: a[0] as u8, p: [a[1] as f32,a[2] as f32,a[3] as f32],
                q: [a[4] as f32,a[5] as f32,a[6] as f32,a[7] as f32],
                half: [a[8] as f32,a[9] as f32,a[10] as f32],class_hash,native_scale,child_of:parent[0] as u8 };
            if !b.valid() { return Vec::new(); }
            boxes.push(b);
        }
        boxes
    }
}

use crate::lua::*;
use crate::native::Native;

/// Per-process scratch of the pose hot path (no allocation per sample once warm).
pub(crate) struct PoseHot {
    buf: Box<PoseBuf>,
    enc: sample::Scratch,
    b: Box<[f64; BONE_NUMS]>,
    w: [[f64; WEAPON_NUMS]; 2],
    c: [f64; CONTROL_NUMS],
    stamped: Vec<u64>,
}

impl Default for PoseHot {
    fn default() -> Self {
        PoseHot {
            buf: PoseBuf::new_boxed(),
            enc: sample::Scratch::default(),
            b: Box::new([0.0; BONE_NUMS]),
            w: [[0.0; WEAPON_NUMS]; 2],
            c: [0.0; CONTROL_NUMS],
            stamped: Vec::new(),
        }
    }
}

fn wall_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// Publish `payload` of `kind` into the game slot `slot` behind `meta`.
fn put_slot(seg: Option<&'static hsmp_ipc::segment::Segment>, slot: &str, meta: hsmp_ipc::schema::SlotMeta, kind: u16, payload: &[u8], scratch: &mut Vec<u64>) -> bool {
    seg.and_then(|s| s.slot_ref(slot, 0)).is_some_and(|sl| sl.put(meta, kind, payload, scratch))
}

/// Why a hot-path write was refused: `NotOpen`, or the record field that failed its check
/// (Lua sees `nil, "bad:<field>"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HotErr {
    NotOpen,
    Bad(&'static str),
}

unsafe fn hot_result(L: *mut lua_State, r: Result<(), HotErr>) -> c_int {
    unsafe {
        match r {
            Ok(()) => {
                lua_pushboolean(L, 1);
                1
            }
            Err(HotErr::NotOpen) => nil_err(L, "not open"),
            Err(HotErr::Bad(f)) => nil_err(L, &format!("bad:{f}")),
        }
    }
}

impl Native {
    // ---- the writers (plain values; the Lua functions below and native sampling call them) ----

    // (write_root / write_weapon / write_pose are also the entry points of native sampling.)
    /// The `root` record of one sample into `local_root`: `rot_pyr_deg` = UE rotator
    /// (pitch, yaw, roll), converted to a quaternion here, once.
    #[allow(dead_code)]
    pub(crate) fn write_root(&mut self, tick: u32, ts_ms: f64, pos: [f64; 3], rot_pyr_deg: [f64; 3], vel: [f64; 3], context: Option<hsmp_pose::posecodec::v2::Context>) -> Result<(), HotErr> {
        let a = [tick as f64, ts_ms, pos[0], pos[1], pos[2], rot_pyr_deg[0], rot_pyr_deg[1], rot_pyr_deg[2], vel[0], vel[1], vel[2]];
        self.write_root_args(&a, context)
    }

    fn write_root_args(&mut self, a: &[f64; 11], context: Option<hsmp_pose::posecodec::v2::Context>) -> Result<(), HotErr> {
        if self.seg().is_none() {
            return Err(HotErr::NotOpen);
        }
        let c=context.filter(|c|c.match_id!=0 && c.round!=0 && c.life!=0).ok_or(HotErr::Bad("context"))?;
        let mut r = sample::root(a, wall_ms());
        r.match_id=c.match_id; r.round=c.round; r.life=c.life;
        hsmp_ipc::record::check(&r, &[]).map_err(|e| HotErr::Bad(e.field().unwrap_or("root")))?;
        let meta = self.meta();
        if !put_slot(self.seg(), "local_root", meta, K_ROOT, bytemuck::bytes_of(&r), &mut self.pose.stamped) {
            return Err(HotErr::NotOpen);
        }
        self.wrote = true;
        Ok(())
    }

    /// The `weapon` record into `local_weapon` (`held`: 0 none, 1 right, 2 left).
    #[allow(dead_code)]
    pub(crate) fn write_weapon(&mut self, tick: u32, ts_ms: f64, weapon_id: u32, held: u32, pos: [f64; 3], rot_pyr_deg: [f64; 3], vel: [f64; 3]) -> Result<(), HotErr> {
        let a = [tick as f64, ts_ms, weapon_id as f64, held as f64, pos[0], pos[1], pos[2], rot_pyr_deg[0], rot_pyr_deg[1], rot_pyr_deg[2], vel[0], vel[1], vel[2]];
        self.write_weapon_args(&a)
    }

    fn write_weapon_args(&mut self, a: &[f64; 13]) -> Result<(), HotErr> {
        if self.seg().is_none() {
            return Err(HotErr::NotOpen);
        }
        let w = sample::weapon(a);
        hsmp_ipc::record::check(&w, &[]).map_err(|e| HotErr::Bad(e.field().unwrap_or("weapon")))?;
        let meta = self.meta();
        if !put_slot(self.seg(), "local_weapon", meta, K_WEAPON, bytemuck::bytes_of(&w), &mut self.pose.stamped) {
            return Err(HotErr::NotOpen);
        }
        self.wrote = true;
        Ok(())
    }

    /// The `pose` record (the codec v2 frame, encoded here once) into `local_pose`: `bones` =
    /// 23 x (p xyz, q xyzw, v xyz, w xyz) world space, `weapons` = up to 2 x 21 numbers
    /// (hands, class, p q v w, blade base xyz, tip xyz; world), `control` = the 37 control
    /// numbers. `k` is ignored (the encoder measures the skeleton scale).
    #[allow(dead_code)]
    pub(crate) fn write_pose(&mut self, tick: u32, ts_ms: f64, dt_ms: f64, k: f64, bones: &[f64; BONE_NUMS], weapons: &[[f64; WEAPON_NUMS]], control: Option<&[f64; CONTROL_NUMS]>, boxes: &[Vec<WeaponBox>], strikers: Option<&[BodyStriker]>, context: Option<hsmp_pose::posecodec::v2::Context>) -> Result<(), HotErr> {
        let _ = k;
        if self.seg().is_none() {
            return Err(HotErr::NotOpen);
        }
        let p = &mut *self.pose;
        let a = PoseArgs { tick: tick as f64, ts: ts_ms, dt: dt_ms, b: bones, w: &weapons[..weapons.len().min(2)], c: control };
        if !sample::encode_pose_with_context(&a, &mut p.enc, &mut p.buf, boxes, strikers, context) {
            return Err(HotErr::Bad("b"));
        }
        self.publish_pose()
    }

    fn publish_pose(&mut self) -> Result<(), HotErr> {
        let (meta, seg) = (self.meta(), self.seg());
        let p = &mut *self.pose;
        if !put_slot(seg, "local_pose", meta, K_POSE, p.buf.payload(), &mut p.stamped) {
            return Err(HotErr::NotOpen);
        }
        self.wrote = true;
        Ok(())
    }

    // ---- the Lua functions -------------------------------------------------------------------

    pub unsafe fn put_root(&mut self, L: *mut lua_State) -> c_int {
        unsafe {
            if self.seg().is_none() {
                return nil_err(L, "not open");
            }
            let mut a = [0f64; 11];
            for (i, v) in a.iter_mut().enumerate() {
                match arg_num(L, i as c_int + 1) {
                    Some(x) => *v = x,
                    None => return nil_err(L, "bad"),
                }
            }
            let context=read_pose_context(L,12);
            let r = self.write_root_args(&a,context);
            hot_result(L, r)
        }
    }

    pub unsafe fn put_weapon(&mut self, L: *mut lua_State) -> c_int {
        unsafe {
            if self.seg().is_none() {
                return nil_err(L, "not open");
            }
            let mut a = [0f64; 13];
            for (i, v) in a.iter_mut().enumerate() {
                match arg_num(L, i as c_int + 1) {
                    Some(x) => *v = x,
                    None => return nil_err(L, "bad"),
                }
            }
            let r = self.write_weapon_args(&a);
            hot_result(L, r)
        }
    }

    /// `put_pose(tick, ts, dt, k, b, w?, c?)`: b = 23 x 13 numbers, w = 21 x n (n <= 2;
    /// more are ignored), c = 37 control numbers. The frame is encoded here, once.
    pub unsafe fn put_pose(&mut self, L: *mut lua_State) -> c_int {
        unsafe {
            if self.seg().is_none() {
                return nil_err(L, "not open");
            }
            let (Some(tick), Some(ts), Some(dt), Some(_k)) = (arg_num(L, 1), arg_num(L, 2), opt_num(L, 3, 0.0), opt_num(L, 4, 0.0)) else {
                return nil_err(L, "bad");
            };
            if !is_table(L, 5) {
                return nil_err(L, "bad");
            }
            let p = &mut *self.pose;
            let bt = lua_absindex(L, 5);
            for chunk in 0..BONE_NUMS / 13 {
                if !geti_nums(L, bt, (chunk * 13 + 1) as i64, &mut p.b[chunk * 13..chunk * 13 + 13]) {
                    return nil_err(L, "bad");
                }
            }
            let mut nw = 0;
            if is_table(L, 6) {
                let wt = lua_absindex(L, 6);
                nw = ((lua_rawlen(L, wt) as usize) / WEAPON_NUMS).min(2);
                for wi in 0..nw {
                    if !geti_nums(L, wt, (wi * WEAPON_NUMS + 1) as i64, &mut p.w[wi]) {
                        return nil_err(L, "bad");
                    }
                }
            }
            let mut has_c = false;
            if is_table(L, 7) {
                let ct = lua_absindex(L, 7);
                if !geti_nums(L, ct, 1, &mut p.c) {
                    return nil_err(L, "bad");
                }
                has_c = true;
            }
            // (`tick` stays an f64 here: the encoder truncates it exactly as before.)
            let mut boxes = Vec::new();
            if is_table(L, 8) {
                for i in 1..=nw {
                    lua_rawgeti(L, 8, i as i64);
                    boxes.push(read_weapon_boxes(L, -1));
                    pop(L, 1);
                }
            }
            let a = PoseArgs { tick, ts, dt, b: &p.b, w: &p.w[..nw], c: has_c.then_some(&p.c) };
            let strikers=read_body_strikers(L,9);
            let context=read_pose_context(L,10);
            if !sample::encode_pose_with_context(&a, &mut p.enc, &mut p.buf, &boxes, strikers.as_deref(), context) {
                return nil_err(L, "bad:b");
            }
            let r = self.publish_pose();
            hot_result(L, r)
        }
    }

    pub unsafe fn put_lead(&mut self, L: *mut lua_State) -> c_int {
        unsafe {
            if self.seg().is_none() {
                return nil_err(L, "not open");
            }
            let Some(ms) = arg_num(L, 1) else { return nil_err(L, "bad") };
            let v = PoseLead { meta: self.meta(), lead_ms: ms as f32, _r: 0 };
            if let Some(s) = self.seg() {
                s.game_out.pose_lead.write(&v);
            }
            self.wrote = true;
            lua_pushboolean(L, 1);
            1
        }
    }
}

