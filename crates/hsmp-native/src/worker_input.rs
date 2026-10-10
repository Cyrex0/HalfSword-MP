//! Native worker input dispatch. Only verified Willie input UFunctions execute;
//! the controller must still possess this exact live pawn for every call.
//! Borrowed engine pointers never survive the Lua entry point.
use hsmp_server::native_wire::{EntityRef, InputFrame};
use std::collections::HashMap;
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
    ("Run", 4, 5),
    ("Crouch Hold", 9, 10),
    ("Thrust", 0, 1),
    ("Jump", 14, 15),
    ("Grab Right", 20, 21),
    ("Grab Left", 18, 19),
    ("Talk", 22, 23),
];
#[derive(Clone, Copy, Default)]
struct BoundFn {
    weak: u64,
    offset: usize,
    size: usize,
}
struct Bindings {
    willie: u64,
    pc: u64,
    axes: [BoundFn; 8],
    buttons: [[BoundFn; 2]; 7],
}
#[derive(Clone, Copy)]
struct EffectiveInput {
    pawn: usize,
    controller: usize,
    buttons: u32,
    axes: [f32; 8],
}
#[derive(Default)]
pub(crate) struct WorkerInputState {
    bindings: Option<Bindings>,
    why: Option<String>,
    executed: HashMap<EntityRef, InputFrame>,
    effective: HashMap<EntityRef, EffectiveInput>,
    inflight: HashMap<EntityRef, (u32, u32, usize, usize)>,
}
impl WorkerInputState {
    pub(crate) fn drop_world(&mut self) {
        self.bindings = None;
        self.why = None;
        self.executed.clear();
        self.effective.clear();
        self.inflight.clear();
    }
    pub(crate) fn execution(&self, reference: EntityRef) -> InputFrame {
        self.executed.get(&reference).copied().unwrap_or_default()
    }
    pub(crate) fn incomplete(&self) -> bool {
        !self.inflight.is_empty()
    }
    pub(crate) fn effective(&self, reference: EntityRef, pawn: usize, controller: usize) -> Option<(u32, [f32; 8])> {
        self.effective.get(&reference).filter(|state| state.pawn == pawn && state.controller == controller)
            .map(|state| (state.buttons, state.axes))
    }
    fn complete_input(&mut self, reference: EntityRef, pawn: usize, controller: usize, buttons: u32, axes: [f32; 8], request: Option<InputFrame>) {
        self.effective.insert(reference, EffectiveInput { pawn, controller, buttons, axes });
        self.inflight.remove(&reference);
        if let Some(request) = request {
            if request.delivery_seq > self.execution(reference).delivery_seq {
                self.executed.insert(reference, request);
            }
        }
    }
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
            if std::env::var("HSMP_RUNTIME_ROLE").as_deref() != Ok("native_worker") {
                return nil_err(L, "role");
            }
            if !self.sample.world_ok || !is_table(L, 1) {
                return nil_err(L, "world");
            }
            let Some(vt) = reflect::vt() else {
                return nil_err(L, "unavailable");
            };
            let table = lua_absindex(L, 1);
            let integer = |name: &str| {
                rawget_str(L, table, name);
                let v = arg_int(L, -1);
                pop(L, 1);
                v
            };
            let (Some(pawn), Some(pc), Some(changed), Some(buttons)) = (
                integer("pawn"),
                integer("controller"),
                integer("changed"),
                integer("buttons"),
            ) else {
                return nil_err(L, "bad");
            };
            if !(0..=127).contains(&changed) || !(0..=127).contains(&buttons) {
                return nil_err(L, "buttons");
            }
            let mut axes = [0.0; 8];
            rawget_str(L, table, "axes");
            let got_axes = is_table(L, -1)
                && lua_rawlen(L, -1) == 8
                && geti_nums(L, lua_absindex(L, -1), 1, &mut axes);
            pop(L, 1);
            if !got_axes
                || axes.iter().enumerate().any(|(i, v)| {
                    !v.is_finite() || v.abs() > if i == 2 || i == 3 { 100.0 } else { 1.0 }
                })
                || axes[4] < 0.0
                || axes[5] < 0.0
            {
                return nil_err(L, "axes");
            }
            let request = if let Some(epoch) = integer("epoch") {
                let (Some(id), Some(incarnation), Some(seq), Some(delivery_seq)) = (
                    integer("id"),
                    integer("incarnation"),
                    integer("seq"),
                    integer("delivery_seq"),
                ) else {
                    return nil_err(L, "gameplay execution identity");
                };
                let (Ok(id), Ok(incarnation), Ok(seq), Ok(delivery_seq)) = (
                    u32::try_from(id),
                    u32::try_from(incarnation),
                    u32::try_from(seq),
                    u32::try_from(delivery_seq),
                ) else {
                    return nil_err(L, "gameplay execution counters");
                };
                let reference = EntityRef {
                    epoch: epoch as u64,
                    id,
                    incarnation,
                };
                if seq == 0
                    || delivery_seq == 0
                    || !self.native_host.directory().is_some_and(|d| {
                        d.entities.iter().any(|e| {
                            e.reference == reference && e.kind == hsmp_server::native_wire::HUMAN
                        })
                    })
                {
                    return nil_err(L, "gameplay execution directory");
                }
                let previous = self.sample.input.execution(reference);
                if delivery_seq < previous.delivery_seq || seq < previous.seq {
                    return nil_err(L, "stale gameplay execution");
                }
                if self.sample.input.inflight.contains_key(&reference) {
                    return nil_err(L, "previous gameplay execution incomplete");
                }
                if delivery_seq == previous.delivery_seq
                    && (seq != previous.seq || changed != 0 || buttons as u32 != previous.buttons)
                {
                    return nil_err(L, "duplicate gameplay edge");
                }
                Some(InputFrame {
                    reference,
                    seq,
                    delivery_seq,
                    buttons: buttons as u32,
                    axes: axes.map(|v| v as f32),
                    ..Default::default()
                })
            } else {
                None
            };
            if self
                .sample
                .input
                .inflight
                .values()
                .any(|(_, _, old_pawn, old_pc)| {
                    *old_pawn == pawn as usize && *old_pc == pc as usize
                })
            {
                return nil_err(L, "previous gameplay execution incomplete");
            }
            if self.sample.input.bindings.is_none() {
                if let Some(why) = &self.sample.input.why {
                    return nil_err(L, why);
                }
                let names = self.sample_names(vt);
                match verify(vt, &names) {
                    Ok(bindings) => self.sample.input.bindings = Some(bindings),
                    Err(error) => {
                        self.sample.input.why = Some(error.clone());
                        return nil_err(L, &error);
                    }
                }
            }
            let names = self.sample_names(vt);
            let bindings = self.sample.input.bindings.as_ref().unwrap();
            let (wc, pcc, axis_functions, button_functions) = (
                bindings.willie,
                bindings.pc,
                bindings.axes,
                bindings.buttons,
            );
            let pawn = pawn as usize as *mut c_void;
            let pc = pc as usize as *mut c_void;
            let original_directory = self.native_host.directory();
            let reference = original_directory.as_ref().and_then(|directory| {
                self.presentation.input_reference(vt, directory, pawn as usize, pc as usize)
            });
            if (std::env::var("HSMP_NATIVE_GAMEPLAY").as_deref() == Ok("1") || request.is_some())
                && (reference.is_none() || request.is_some_and(|r| Some(r.reference) != reference)) {
                return nil_err(L, "gameplay source input binding unavailable");
            }
            let admitted = || {
                live(vt, pawn, wc).is_some()
                    && live(vt, pc, pcc).is_some()
                    && pointer_property(vt, &names, pc, "Pawn") == Some(pawn)
                    && pointer_property(vt, &names, pawn, "Controller") == Some(pc)
                    && reference.is_none_or(|expected| {
                        self.native_host.directory().as_ref().zip(original_directory.as_ref()).is_some_and(|(current, original)| {
                            current.epoch == original.epoch && current.seq == original.seq && current.entities == original.entities
                        })
                            && original_directory.as_ref().and_then(|directory| self.presentation.input_reference(vt, directory, pawn as usize, pc as usize)) == Some(expected)
                    })
            };
            if !admitted() { return nil_err(L, "possession"); }
            if let Some(reference) = reference {
                let acknowledgement = request.unwrap_or_else(|| self.sample.input.execution(reference));
                self.sample.input.inflight.insert(
                    reference,
                    (
                        acknowledgement.seq,
                        acknowledgement.delivery_seq,
                        pawn as usize,
                        pc as usize,
                    ),
                );
            }
            let mut params = Params([0; crate::sample::PARAMS_BYTES]);
            for (index, function) in axis_functions.into_iter().enumerate() {
                if !self.sample.world_ok || !admitted() {
                    return nil_err(L, "possession");
                }
                let function_object = reflect::get(vt, function.weak);
                if function_object.is_null() {
                    return nil_err(L, "function");
                }
                params.0[..function.size].fill(0);
                params.0[function.offset..function.offset + 4]
                    .copy_from_slice(&(axes[index] as f32).to_le_bytes());
                (vt.call)(pawn, function_object, params.0.as_mut_ptr() as *mut c_void);
                if !self.sample.world_ok || !admitted() {
                    return nil_err(L, "possession");
                }
            }
            for (index, functions) in button_functions.into_iter().enumerate() {
                if changed & (1 << index) == 0 {
                    continue;
                }
                if !self.sample.world_ok || !admitted() {
                    return nil_err(L, "possession");
                }
                let edge = if buttons & (1 << index) != 0 { 0 } else { 1 };
                let function = functions[edge];
                let function_object = reflect::get(vt, function.weak);
                if function_object.is_null() {
                    return nil_err(L, "function");
                }
                // SDK copies only FKey.KeyName; None and all remaining bytes zero
                // match a default FKey without retaining shared-pointer storage.
                params.0[..function.size].fill(0);
                (vt.call)(pawn, function_object, params.0.as_mut_ptr() as *mut c_void);
                if !self.sample.world_ok || !admitted() {
                    return nil_err(L, "possession");
                }
            }
            if let Some(reference) = reference {
                self.sample.input.complete_input(reference, pawn as usize, pc as usize, buttons as u32, axes.map(|value| value as f32), request);
            }
            lua_pushboolean(L, 1);
            1
        }
    }
}

#[cfg(test)]
mod gameplay_input_state_tests {
    use super::*;
    #[test]
    fn successful_neutral_release_updates_effective_state_without_new_acknowledgement() {
        let reference = EntityRef { epoch: 9, id: 1, incarnation: 3 };
        let mut input = WorkerInputState::default();
        let mut axes = [0.0; 8]; axes[0] = 1.0;
        let request = InputFrame { reference, seq: 7, delivery_seq: 11, buttons: 1, axes, ..Default::default() };
        input.complete_input(reference, 100, 200, 1, axes, Some(request));
        assert_eq!(input.effective(reference, 100, 200), Some((1, axes)));
        input.inflight.insert(reference, (7, 11, 100, 200));
        input.complete_input(reference, 100, 200, 0, [0.0; 8], None);
        assert_eq!(input.execution(reference), request);
        assert_eq!(input.effective(reference, 100, 200), Some((0, [0.0; 8])));
        assert_eq!(input.effective(reference, 101, 200), None);
        assert!(!input.incomplete());
        input.drop_world();
        assert_eq!(input.effective(reference, 100, 200), None);
    }
}
