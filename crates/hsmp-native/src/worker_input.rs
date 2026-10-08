//! Native worker input dispatch. Only verified Willie input UFunctions execute;
//! the controller must still possess this exact live pawn for every call.
//! Borrowed engine pointers never survive the Lua entry point.
use std::ffi::{c_int, c_void};

use crate::lua::*;
use crate::native::Native;
use crate::reflect::{self, wide, HsmpProp, HsmpReflect};
use crate::sample::{fname, live, props_of, Names, Params};

const PREFIX: &str = "/Game/Character/Blueprints/Willie_BP.Willie_BP_C:";
const AXES: [&str; 8] = [
    "InpAxisEvt_Move Forward / Backward_K2Node_InputAxisEvent_14",
    "InpAxisEvt_Move Right / Left_K2Node_InputAxisEvent_19",
    "InpAxisEvt_Turn Right / Left Mouse_K2Node_InputAxisEvent_16",
    "InpAxisEvt_Look Up / Down Mouse_K2Node_InputAxisEvent_17",
    "InpAxisEvt_Right Guard Axis_K2Node_InputAxisEvent_6",
    "InpAxisEvt_Left Guard Axis_K2Node_InputAxisEvent_7",
    "InpAxisEvt_Right Arm Axis_K2Node_InputAxisEvent_2",
    "InpAxisEvt_Left Arm Axis_K2Node_InputAxisEvent_3",
];
// Exact press/release branches, verified in the cooked Willie ubergraph.
const BUTTONS: [(&str, u32, u32); 7] = [
    ("Run", 4, 5), ("Crouch Hold", 9, 10), ("Thrust", 0, 1),
    ("Jump", 14, 15), ("Grab Right", 20, 21),
    ("Grab Left", 18, 19), ("Talk", 22, 23),
];
#[derive(Clone, Copy, Default)]
struct BoundFn { weak: u64, offset: usize, size: usize }
struct Bindings { willie: u64, pc: u64, axes: [BoundFn; 8], buttons: [[BoundFn; 2]; 7] }
#[derive(Default)]
pub(crate) struct WorkerInputState { bindings: Option<Bindings>, why: Option<String> }
impl WorkerInputState {
    pub(crate) fn drop_world(&mut self) { self.bindings = None; self.why = None; }
}

unsafe fn verify_function(vt: &HsmpReflect, n: &Names, path: &str, key: bool) -> Result<BoundFn, String> {
    unsafe {
        let function = (vt.find)(wide(path).as_ptr());
        if function.is_null() { return Err(format!("missing {path}")); }
        let (props, size) = props_of(vt, function);
        let Some(prop) = props.first() else { return Err(format!("params {path}")); };
        let valid = if key {
            size == 24 && prop.name == fname(vt, "Key", true) && prop.cls == n.struct_prop
                && prop.sub == fname(vt, "Key", true) && prop.size == 24
        } else {
            size == 4 && prop.name == fname(vt, "AxisValue", true) && prop.cls == n.float_prop && prop.size == 4
        };
        if props.len() != 1 || prop.offset != 0 || !valid { return Err(format!("ABI {path}")); }
        Ok(BoundFn { weak: reflect::keep(vt, function).ok_or_else(|| format!("weak {path}"))?, offset: 0, size: size as usize })
    }
}
unsafe fn verify(vt: &HsmpReflect, n: &Names) -> Result<Bindings, String> {
    unsafe {
        let key = (vt.find)(wide("/Script/InputCore.Key").as_ptr());
        if key.is_null() { return Err("missing FKey".into()); }
        let (props, size) = props_of(vt, key);
        if size != 24 || props.len() != 1 || props[0].name != fname(vt, "KeyName", true)
            || props[0].cls != n.name_prop || props[0].size != 8 || props[0].offset != 0 { return Err("FKey ABI".into()); }
        let keep_class = |path: &str| -> Result<u64, String> {
            let object = (vt.find)(wide(path).as_ptr());
            if object.is_null() { return Err(format!("missing {path}")); }
            reflect::keep(vt, object).ok_or_else(|| format!("weak {path}"))
        };
        let mut bindings = Bindings {
            willie: keep_class("/Game/Character/Blueprints/Willie_BP.Willie_BP_C")?,
            pc: keep_class("/Script/Engine.PlayerController")?,
            axes: [BoundFn::default(); 8], buttons: [[BoundFn::default(); 2]; 7],
        };
        for (index, name) in AXES.iter().enumerate() {
            bindings.axes[index] = verify_function(vt, n, &format!("{PREFIX}{name}"), false)?;
        }
        for (index, (name, press, release)) in BUTTONS.iter().enumerate() {
            for (edge, suffix) in [press, release].into_iter().enumerate() {
                bindings.buttons[index][edge] = verify_function(vt, n, &format!("{PREFIX}InpActEvt_{name}_K2Node_InputActionEvent_{suffix}"), true)?;
            }
        }
        Ok(bindings)
    }
}
unsafe fn pointer_property(vt: &HsmpReflect, n: &Names, object: *mut c_void, field: &str) -> Option<*mut c_void> {
    unsafe {
        let mut prop = HsmpProp::default();
        if (vt.obj_prop)(object, wide(field).as_ptr(), &mut prop) != 1 || prop.cls != n.object_prop
            || prop.size != 8 || prop.offset < 0 { return None; }
        let bytes = std::slice::from_raw_parts((object as *const u8).add(prop.offset as usize), 8);
        Some(u64::from_le_bytes(bytes.try_into().ok()?) as usize as *mut c_void)
    }
}
impl Native {
    /// `worker_input({pawn, controller, axes[8], changed, buttons}) -> true | nil,error`.
    /// Mouse axes are deltas already consumed exactly once by the Lua controller.
    pub unsafe fn worker_input(&mut self, L: *mut lua_State) -> c_int {
        unsafe {
            if std::env::var("HSMP_RUNTIME_ROLE").as_deref() != Ok("native_worker") { return nil_err(L, "role"); }
            if !self.sample.world_ok || !is_table(L, 1) { return nil_err(L, "world"); }
            let Some(vt) = reflect::vt() else { return nil_err(L, "unavailable"); };
            let table = lua_absindex(L, 1);
            let integer = |name: &str| { rawget_str(L, table, name); let v = arg_int(L, -1); pop(L, 1); v };
            let (Some(pawn), Some(pc), Some(changed), Some(buttons)) =
                (integer("pawn"), integer("controller"), integer("changed"), integer("buttons")) else { return nil_err(L, "bad"); };
            if !(0..=127).contains(&changed) || !(0..=127).contains(&buttons) { return nil_err(L, "buttons"); }
            let mut axes = [0.0; 8];
            rawget_str(L, table, "axes");
            let got_axes = is_table(L, -1) && lua_rawlen(L, -1) == 8 && geti_nums(L, lua_absindex(L, -1), 1, &mut axes);
            pop(L, 1);
            if !got_axes || axes.iter().enumerate().any(|(i, v)| !v.is_finite() || v.abs() > if i == 2 || i == 3 { 100.0 } else { 1.0 })
                || axes[4] < 0.0 || axes[5] < 0.0 { return nil_err(L, "axes"); }
            if self.sample.input.bindings.is_none() {
                if let Some(why) = &self.sample.input.why { return nil_err(L, why); }
                let names = self.sample_names(vt);
                match verify(vt, &names) {
                    Ok(bindings) => self.sample.input.bindings = Some(bindings),
                    Err(error) => { self.sample.input.why = Some(error.clone()); return nil_err(L, &error); }
                }
            }
            let names = self.sample_names(vt);
            let bindings = self.sample.input.bindings.as_ref().unwrap();
            let (wc, pcc, axis_functions, button_functions) = (bindings.willie, bindings.pc, bindings.axes, bindings.buttons);
            let pawn = pawn as usize as *mut c_void;
            let pc = pc as usize as *mut c_void;
            let admitted = || {
                live(vt, pawn, wc).is_some() && live(vt, pc, pcc).is_some()
                    && pointer_property(vt, &names, pc, "Pawn") == Some(pawn)
                    && pointer_property(vt, &names, pawn, "Controller") == Some(pc)
            };
            let mut params = Params([0; crate::sample::PARAMS_BYTES]);
            for (index, function) in axis_functions.into_iter().enumerate() {
                if !self.sample.world_ok || !admitted() { return nil_err(L, "possession"); }
                let function_object = reflect::get(vt, function.weak);
                if function_object.is_null() { return nil_err(L, "function"); }
                params.0[..function.size].fill(0);
                params.0[function.offset..function.offset + 4].copy_from_slice(&(axes[index] as f32).to_le_bytes());
                (vt.call)(pawn, function_object, params.0.as_mut_ptr() as *mut c_void);
            }
            for (index, functions) in button_functions.into_iter().enumerate() {
                if changed & (1 << index) == 0 { continue; }
                if !self.sample.world_ok || !admitted() { return nil_err(L, "possession"); }
                let edge = if buttons & (1 << index) != 0 { 0 } else { 1 };
                let function = functions[edge];
                let function_object = reflect::get(vt, function.weak);
                if function_object.is_null() { return nil_err(L, "function"); }
                // SDK copies only FKey.KeyName; None and all remaining bytes zero
                // match a default FKey without retaining shared-pointer storage.
                params.0[..function.size].fill(0);
                (vt.call)(pawn, function_object, params.0.as_mut_ptr() as *mut c_void);
            }
            lua_pushboolean(L, 1); 1
        }
    }
}
