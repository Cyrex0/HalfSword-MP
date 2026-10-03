//! The engine-reflection vtable for native sampling / servo. The UE4SS C++ mod
//! (`cpp/src/ue4ss_reflect.cpp`) resolves a handful of UE4SS.dll exports at start and hands
//! this table over with [`hsmp_native_set_reflect`]; option E (`hsmp_lua.dll`) and the cargo
//! tests without a fake never do, so native sampling reports `unavailable` there.
//!
//! Every pointer here is a raw engine pointer that is only valid for the duration of the
//! Lua -> native call that obtained it (rule 2 in the `sample.rs` header): long-lived references are `weak` handles
//! (object index + serial number, read off and resolved through UE4SS's GUObjectArray
//! exports; a kept handle without a serial is also pinned to its address: [`keep`], [`get`]).

use std::ffi::c_void;
use std::sync::atomic::{AtomicPtr, Ordering};

/// ABI version of [`HsmpReflect`] (C: `HSMP_REFLECT_ABI`).
pub const REFLECT_ABI: u32 = 1;

/// One reflected property (`FProperty`), as the C++ side describes it. Names are FNames as
/// 8 bytes (ComparisonIndex | Number << 32), compared as integers.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HsmpProp {
    /// The property's FName.
    pub name: u64,
    /// The property class's FName ("StructProperty", "NameProperty", ...).
    pub cls: u64,
    /// StructProperty: the struct's FName ("Vector", "Transform", ...); otherwise 0.
    pub sub: u64,
    /// Offset in the container (object / params / struct).
    pub offset: i32,
    /// `ElementSize`.
    pub size: i32,
    /// BoolProperty: byte offset and mask of the bit (0 / 0 otherwise).
    pub bool_offset: u8,
    pub bool_mask: u8,
    pub _r: [u8; 6],
}

/// The functions the C++ mod provides. All of them run on the game thread inside a Lua call.
#[repr(C)]
pub struct HsmpReflect {
    pub abi: u32,
    pub _r: u32,
    /// FName of a UTF-16 NUL-terminated string (`add` = FNAME_Add, else FNAME_Find; 0 = none).
    pub fname: unsafe extern "C" fn(s: *const u16, add: i32) -> u64,
    /// `StaticFindObject(nullptr, nullptr, path, false)`.
    pub find: unsafe extern "C" fn(path: *const u16) -> *mut c_void,
    /// `obj->IsA(cls)`.
    pub is_a: unsafe extern "C" fn(obj: *mut c_void, cls: *mut c_void) -> i32,
    /// The UClass of `obj`.
    pub class_of: unsafe extern "C" fn(obj: *mut c_void) -> *mut c_void,
    /// The properties of a UStruct / UFunction (its params) into `out[..cap]`; returns their
    /// count (may exceed `cap`), `*size` = the struct's properties size.
    pub props: unsafe extern "C" fn(ustruct: *mut c_void, out: *mut HsmpProp, cap: i32, size: *mut i32) -> i32,
    /// `obj->GetPropertyByNameInChain(name)` described into `out`; 1 if found.
    pub obj_prop: unsafe extern "C" fn(obj: *mut c_void, name: *const u16, out: *mut HsmpProp) -> i32,
    /// `obj->ProcessEvent(func, params)`.
    pub call: unsafe extern "C" fn(obj: *mut c_void, func: *mut c_void, params: *mut c_void),
    /// A weak handle `{InternalIndex, SerialNumber}` (FWeakObjectPtr's layout) read off the
    /// object array, never through UE4SS's FWeakObjectPtr (its ctor allocates a serial for an
    /// object without one and faults on this build). 0 = not in the array. Serial 0 (the
    /// object has none) proves nothing alone: kept handles go through [`keep`] / [`get`].
    pub weak: unsafe extern "C" fn(obj: *mut c_void) -> u64,
    /// The object in a weak handle's slot: null if the slot holds another serial (a nonzero
    /// handle serial only) or the object is pending kill / unreachable.
    pub resolve: unsafe extern "C" fn(weak: u64) -> *mut c_void,
}

/// A weak handle whose serial alone proves identity (nonzero with a nonzero serial).
pub fn persistent(w: u64) -> bool {
    w != 0 && (w >> 32) != 0
}

/// Kept serial-0 handles -> the object address seen when they were made. Most engine objects
/// never get a serial (in-game, even every /Script UFunction and class had none), so a kept
/// handle without one is identified by its slot AND that address: compared, never
/// dereferenced. Game thread only (the lock is uncontended).
static PINNED: std::sync::Mutex<Option<std::collections::HashMap<u64, usize>>> = std::sync::Mutex::new(None);

/// A handle of `obj` to keep across calls, or None (not in the object array).
///
/// # Safety
/// `obj` live (found / proven in this call); game thread.
pub unsafe fn keep(vt: &HsmpReflect, obj: *mut c_void) -> Option<u64> {
    // SAFETY: as the caller guarantees.
    let w = unsafe { (vt.weak)(obj) };
    if w == 0 {
        return None;
    }
    if !persistent(w) {
        let mut g = PINNED.lock().unwrap_or_else(|e| e.into_inner());
        g.get_or_insert_with(Default::default).insert(w, obj as usize);
    }
    Some(w)
}

/// The object of a kept handle ([`keep`]), or null if it is gone (another serial / another
/// object in the slot, pending kill, unreachable).
///
/// # Safety
/// Game thread.
pub unsafe fn get(vt: &HsmpReflect, w: u64) -> *mut c_void {
    // SAFETY: resolve only reads the object array.
    let p = unsafe { (vt.resolve)(w) };
    if p.is_null() || persistent(w) {
        return p;
    }
    let g = PINNED.lock().unwrap_or_else(|e| e.into_inner());
    match g.as_ref().and_then(|m| m.get(&w)) {
        Some(&q) if q == p as usize => p,
        _ => std::ptr::null_mut(),
    }
}

static VT: AtomicPtr<HsmpReflect> = AtomicPtr::new(std::ptr::null_mut());

/// Install the reflection vtable (the C++ mod, once, from `start_mod`; tests). A table with
/// another ABI version is ignored. Null removes it.
///
/// # Safety
/// `vt` null or valid for the life of the process.
#[no_mangle]
pub unsafe extern "C" fn hsmp_native_set_reflect(vt: *const HsmpReflect) {
    // SAFETY: the caller guarantees `vt` is null or valid forever.
    let ok = vt.is_null() || unsafe { (*vt).abi } == REFLECT_ABI;
    if ok {
        VT.store(vt as *mut HsmpReflect, Ordering::Release);
    }
}

/// The installed vtable, if any.
pub fn vt() -> Option<&'static HsmpReflect> {
    let p = VT.load(Ordering::Acquire);
    // SAFETY: set only by `hsmp_native_set_reflect`, valid for the life of the process.
    (!p.is_null()).then(|| unsafe { &*p })
}

/// A NUL-terminated UTF-16 copy of `s`.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}
