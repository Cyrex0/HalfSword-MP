//! Rare source-harvest identities. Stored state contains scalars, never borrowed
//! UObjects. Serial zero uses original slot/address/FName/class identity exactly
//! as the presentation provider does; no FWeakObjectPtr serial is allocated.
use crate::{
    lua::*,
    native::Native,
    reflect::{self, HsmpReflect},
};
use hsmp_server::native_wire::EntityRef;
use std::{
    cell::RefCell,
    collections::HashMap,
    ffi::{c_int, c_void},
    rc::Rc,
    sync::atomic::{AtomicPtr, Ordering},
    time::Instant,
};

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ProfileTrace {
    epoch: u64,
    handle: u64,
    guards: u64,
    admissions: u64,
    finds: u64,
    events: u64,
    elapsed_us: u64,
    entity: u32,
    incarnation: u32,
    dir_seq: u32,
    seq: u32,
}
type ProfileLogger = unsafe extern "C" fn(*const std::ffi::c_char, u32, *const ProfileTrace);
static PROFILE_LOGGER: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
#[derive(Clone, Copy)]
struct ProfileSession {
    trace: ProfileTrace,
    start: Instant,
    progress: u32,
    once: u32,
}
thread_local! {static PROFILE_TRACE:RefCell<Option<ProfileSession>>=const{RefCell::new(None)};}
struct ProfileOperation {
    previous: Option<ProfileSession>,
}
impl ProfileOperation {
    fn begin(reference: EntityRef, dir_seq: u32, handle: u64) -> Self {
        let session = ProfileSession {
            trace: ProfileTrace {
                epoch: reference.epoch,
                entity: reference.id,
                incarnation: reference.incarnation,
                dir_seq,
                handle,
                ..Default::default()
            },
            start: Instant::now(),
            progress: 0,
            once: 0,
        };
        let previous = PROFILE_TRACE.with(|p| p.replace(Some(session)));
        profile_checkpoint(c"profile_operation", 0);
        Self { previous }
    }
}
impl Drop for ProfileOperation {
    fn drop(&mut self) {
        profile_checkpoint(c"profile_operation", 1);
        PROFILE_TRACE.with(|p| p.replace(self.previous));
    }
}
#[no_mangle]
pub extern "C" fn hsmp_native_set_profile_logger(logger: Option<ProfileLogger>) {
    PROFILE_LOGGER.store(
        logger.map_or(std::ptr::null_mut(), |p| p as *mut c_void),
        Ordering::Release,
    );
}
fn profile_checkpoint(stage: &std::ffi::CStr, edge: u32) {
    if stage.to_bytes().len() > 64
        || edge > 2
        || !stage
            .to_bytes()
            .iter()
            .all(|c| c.is_ascii_alphanumeric() || *c == b'_')
    {
        return;
    }
    let snapshot = PROFILE_TRACE.with(|p| {
        let mut p = p.borrow_mut();
        let s = p.as_mut()?;
        if s.trace.seq >= 64 {
            return None;
        }
        let limited = s.trace.seq == 63;
        s.trace.seq += 1;
        s.trace.elapsed_us = s.start.elapsed().as_micros().min(u64::MAX as u128) as u64;
        Some((s.trace, limited))
    });
    // No RefCell borrow or Native/schema lock survives the logger callback.
    let logger = PROFILE_LOGGER.load(Ordering::Acquire);
    if let (Some((snapshot, limited)), false) = (snapshot, logger.is_null()) {
        let logger: ProfileLogger = unsafe { std::mem::transmute(logger) };
        let (stage, edge) = if limited {
            (c"diagnostic_budget_exhausted", 2)
        } else {
            (stage, edge)
        };
        unsafe { logger(stage.as_ptr(), edge, &snapshot) }
    }
}
fn profile_once(bit: u32, stage: &std::ffi::CStr, edge: u32) {
    let enabled = PROFILE_TRACE.with(|p| {
        let mut p = p.borrow_mut();
        let Some(s) = p.as_mut() else {
            return false;
        };
        if s.once & bit != 0 {
            return false;
        }
        s.once |= bit;
        true
    });
    if enabled {
        profile_checkpoint(stage, edge);
    }
}
#[no_mangle]
pub unsafe extern "C" fn hsmp_native_profile_checkpoint(stage: *const std::ffi::c_char, edge: u32) {
    if !stage.is_null() {
        profile_checkpoint(unsafe { std::ffi::CStr::from_ptr(stage) }, edge);
    }
}
#[no_mangle]
pub extern "C" fn hsmp_native_profile_tick(counter: u32) {
    let progress = PROFILE_TRACE.with(|p| {
        let mut p = p.borrow_mut();
        let Some(s) = p.as_mut() else {
            return false;
        };
        let value = match counter {
            0 => &mut s.trace.guards,
            1 => &mut s.trace.admissions,
            2 => &mut s.trace.finds,
            3 => &mut s.trace.events,
            _ => return false,
        };
        *value = value.saturating_add(1);
        if counter == 1 && s.trace.admissions % 2048 == 0 && s.progress < 8 {
            s.progress += 1;
            true
        } else {
            false
        }
    });
    if progress {
        profile_checkpoint(c"guard_progress", 2);
    }
}

const MAX_COMPONENTS: usize = 64;
// Pawn: Controller, RootComponent, seven weapon fields; controller: Pawn;
// seven weapon roots; at most 64 original component-class AttachParent fields.
const MAX_SCHEMA_FIELDS: usize = MAX_COMPONENTS + 17;
const GARBAGE: u32 = 0x40000000; // matched shipping Kismet validity / actor iterator
#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct PathNode {
    weak: u64,
    address: u64,
    name: u64,
    class_weak: u64,
    class_address: u64,
    class_name: u64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct PathWitness {
    nodes: Vec<PathNode>,
    package_name: u64,
}
type PathReader = unsafe extern "C" fn(
    *const PathNode,
    u32,
    u32,
    *mut PathNode,
    u32,
    *mut u32,
    *mut u64,
    *mut std::ffi::c_char,
    u32,
) -> i32;
static PATH_READER: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
type ObjectFactory = unsafe extern "C" fn(*mut lua_State, u64, *mut std::ffi::c_char, u32) -> i32;
static OBJECT_FACTORY: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
#[no_mangle]
pub extern "C" fn hsmp_native_set_source_object_factory(factory: Option<ObjectFactory>) {
    OBJECT_FACTORY.store(
        factory.map_or(std::ptr::null_mut(), |p| p as *mut c_void),
        Ordering::Release,
    );
}
fn object_factory() -> Result<ObjectFactory, String> {
    let p = OBJECT_FACTORY.load(Ordering::Acquire);
    if p.is_null() {
        Err("source wrapper factory unavailable".into())
    } else {
        Ok(unsafe { std::mem::transmute(p) })
    }
}
#[no_mangle]
pub extern "C" fn hsmp_native_set_source_path_reader(reader: Option<PathReader>) {
    PATH_READER.store(
        reader.map_or(std::ptr::null_mut(), |p| p as *mut c_void),
        Ordering::Release,
    );
}
fn path_reader() -> Result<PathReader, String> {
    let p = PATH_READER.load(Ordering::Acquire);
    if p.is_null() {
        Err("source original path reader unavailable".into())
    } else {
        Ok(unsafe { std::mem::transmute(p) })
    }
}
fn path_error(reason: &[u8; 192]) -> String {
    let n = reason.iter().position(|c| *c == 0).unwrap_or(reason.len());
    format!(
        "source original path: {}",
        String::from_utf8_lossy(&reason[..n])
    )
}
type Name = unsafe extern "C" fn(*const c_void) -> *const u64;
type Flags = unsafe extern "C" fn(*const c_void) -> *const u32;
type World = unsafe extern "C" fn(*const c_void) -> *mut c_void;
#[derive(Clone, Copy)]
struct Exports {
    name: Name,
    flags: Flags,
    world: World,
}
#[cfg(windows)]
fn exports() -> Result<Exports, String> {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetModuleHandleW(name: *const u16) -> *mut c_void;
        fn GetProcAddress(module: *mut c_void, name: *const i8) -> *mut c_void;
    }
    unsafe {
        let module = GetModuleHandleW(reflect::wide("UE4SS.dll").as_ptr());
        if module.is_null() {
            return Err("source native metadata module unavailable".into());
        }
        let get = |s: &std::ffi::CStr| -> Result<*mut c_void, String> {
            let p = GetProcAddress(module, s.as_ptr());
            if module.is_null() || p.is_null() {
                Err("source native metadata exports unavailable".into())
            } else {
                Ok(p)
            }
        };
        Ok(Exports {
            name: std::mem::transmute::<*mut c_void, Name>(get(
                c"?GetNamePrivate@UObjectBase@Unreal@RC@@QEBAAEBVFName@23@XZ",
            )?),
            flags: std::mem::transmute::<*mut c_void, Flags>(get(
                c"?GetObjectFlags@UObjectBase@Unreal@RC@@QEBAAEBW4EObjectFlags@23@XZ",
            )?),
            world: std::mem::transmute::<*mut c_void, World>(get(
                c"?GetWorld@UObject@Unreal@RC@@QEBAPEAVUWorld@23@XZ",
            )?),
        })
    }
}
#[cfg(not(windows))]
fn exports() -> Result<Exports, String> {
    Err("source native metadata exports unavailable".into())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Identity {
    weak: u64,
    address: u64,
    name: u64,
    class_weak: u64,
    class_address: u64,
}
// This trait also lets offline tests exercise the identity/link protocol without
// pretending the fixture is an engine snapshot.
trait Engine {
    fn admit(&self) -> Result<(), String> {
        Ok(())
    }
    fn capture(&self, address: u64) -> Result<Identity, String>;
    fn verify(&self, id: Identity) -> Result<(), String>;
    fn world(&self, id: Identity) -> Result<u64, String>;
    fn field(&self, id: Identity, name: &str) -> Result<u64, String>;
    fn owner(&self, id: Identity) -> Result<u64, String>;
    fn find(&self, path: &str) -> Result<u64, String>;
    fn capture_path(&self, _: Identity) -> Result<Option<PathWitness>, String> {
        Ok(None)
    }
    fn path_matches(
        &self,
        id: Identity,
        path: &str,
        _: Option<&PathWitness>,
    ) -> Result<bool, String> {
        Ok(self.find(path)? == id.address)
    }
}
trait SchemaEngine: Engine {
    fn schema_name(&self, name: &str) -> Result<u64, String>;
    fn property(&self, id: Identity, name: &str) -> Result<reflect::HsmpProp, String>;
    fn parameters(&self, function: Identity) -> Result<(Vec<reflect::HsmpProp>, i32), String>;
    fn read_pointer(&self, id: Identity, property: reflect::HsmpProp) -> Result<u64, String>;
    fn dispatch_owner(
        &self,
        id: Identity,
        function: Identity,
        class: Identity,
    ) -> Result<u64, String>;
}
#[derive(Clone, Copy)]
struct OwnerSchema {
    function: Identity,
    class: Identity,
    result: reflect::HsmpProp,
}
struct FieldSchema {
    class: Identity,
    name: String,
    property: reflect::HsmpProp,
}
#[derive(Default)]
struct SchemaCache {
    owner: Option<OwnerSchema>,
    fields: Vec<FieldSchema>,
}
// Only copied identities/layouts live in the original scope. Cache borrows end
// before identity checks or engine calls; callback reentry never holds one.
fn cached_field(
    cache: &RefCell<SchemaCache>,
    e: &impl SchemaEngine,
    id: Identity,
    name: &str,
) -> Result<u64, String> {
    e.verify(id)?;
    let original_class = {
        let cache = cache.borrow();
        cache
            .fields
            .iter()
            .find(|f| f.class.weak == id.class_weak && f.class.address == id.class_address)
            .map(|f| f.class)
    };
    let class = match original_class {
        Some(class) => class,
        None => e.capture(id.class_address)?,
    };
    if class.weak != id.class_weak || class.address != id.class_address {
        return Err("source scope original property class changed".into());
    }
    e.verify(class)?;
    let property = {
        let cache = cache.borrow();
        cache
            .fields
            .iter()
            .find(|f| f.class == class && f.name == name)
            .map(|f| f.property)
    };
    let property = match property {
        Some(property) => property,
        None => {
            let property = e.property(id, name)?;
            if property.name != e.schema_name(name)?
                || property.cls != e.schema_name("ObjectProperty")?
                || property.size != 8
                || !(0..=65528).contains(&property.offset)
            {
                return Err(format!("source scope hard {name} property ABI"));
            }
            e.verify(class)?;
            e.verify(id)?;
            let mut cache = cache.borrow_mut();
            if cache.fields.len() >= MAX_SCHEMA_FIELDS {
                return Err("source scope property schema bound".into());
            }
            cache.fields.push(FieldSchema {
                class,
                name: name.to_owned(),
                property,
            });
            property
        }
    };
    // The pointer VALUE is never cached. Check the original class on both sides
    // of every freshly read link, including serial-zero native class identities.
    e.verify(class)?;
    let result = e.read_pointer(id, property)?;
    e.verify(id)?;
    e.verify(class)?;
    Ok(result)
}
fn cached_owner(
    cache: &RefCell<SchemaCache>,
    e: &impl SchemaEngine,
    id: Identity,
) -> Result<u64, String> {
    e.verify(id)?;
    let existing = cache.borrow().owner;
    let schema = match existing {
        Some(schema) => schema,
        None => {
            let function = e.capture(e.find("/Script/Engine.ActorComponent:GetOwner")?)?;
            let class = e.capture(e.find("/Script/Engine.ActorComponent")?)?;
            let (properties, size) = e.parameters(function)?;
            if size != 8
                || properties.len() != 1
                || properties[0].name != e.schema_name("ReturnValue")?
                || properties[0].cls != e.schema_name("ObjectProperty")?
                || properties[0].offset != 0
                || properties[0].size != 8
            {
                return Err("source scope GetOwner ABI".into());
            }
            e.verify(function)?;
            e.verify(class)?;
            let schema = OwnerSchema {
                function,
                class,
                result: properties[0],
            };
            cache.borrow_mut().owner = Some(schema);
            schema
        }
    };
    // The checked native signature is immutable for this original class/function
    // in the cooked scope. It is discarded with the scope, never with a new ref.
    debug_assert_eq!(schema.result.offset, 0);
    e.verify(schema.function)?;
    e.verify(schema.class)?;
    e.verify(id)?;
    let result = e.dispatch_owner(id, schema.function, schema.class)?;
    e.verify(id)?;
    e.verify(schema.function)?;
    e.verify(schema.class)?;
    Ok(result)
}
#[derive(Clone)]
struct Owner {
    object: Identity,
    root: Identity,
    field: Option<String>,
}
#[derive(Clone)]
struct Component {
    object: Identity,
    owner: u64,
    parent: Option<Parent>,
    path: String,
    path_witness: Option<PathWitness>,
}
#[derive(Clone, Copy, PartialEq, Eq)]
struct Parent {
    object: Identity,
    owner: u64,
    attach_parent: u64,
}
struct Scope {
    id: u64,
    reference: EntityRef,
    dir_seq: u32,
    index: u8,
    key: Vec<u8>,
    world: Identity,
    pawn: Identity,
    controller: Identity,
    owners: HashMap<u64, Owner>,
    components: Vec<Component>,
    schema: Rc<RefCell<SchemaCache>>,
}
impl Scope {
    // This final read region must not call world/owner getters. It uses only
    // original identities and freshly read hard links from cached schemas.
    fn pure_hard_links(&self, e: &impl Engine, handle: u64) -> Result<(), String> {
        for id in [self.world, self.pawn, self.controller] {
            e.verify(id)?;
        }
        if e.field(self.pawn, "Controller")? != self.controller.address
            || e.field(self.controller, "Pawn")? != self.pawn.address
        {
            return Err("source static vertex pawn/controller link changed".into());
        }
        let c = self
            .components
            .get(handle.checked_sub(1).ok_or("source scope handle")? as usize)
            .ok_or("source scope handle")?;
        let owner_check = |address| -> Result<(), String> {
            let owner = self
                .owners
                .get(&address)
                .ok_or("source static vertex original owner")?;
            e.verify(owner.object)?;
            e.verify(owner.root)?;
            if e.field(owner.object, "RootComponent")? != owner.root.address
                || owner
                    .field
                    .as_ref()
                    .is_some_and(|field| e.field(self.pawn, field).ok() != Some(address))
            {
                return Err("source static vertex original owner/root/weapon link changed".into());
            }
            Ok(())
        };
        owner_check(c.owner)?;
        e.verify(c.object)?;
        if e.field(c.object, "AttachParent")? != c.parent.map_or(0, |p| p.object.address) {
            return Err("source static vertex original component parent link changed".into());
        }
        if let Some(parent) = c.parent {
            e.verify(parent.object)?;
            owner_check(parent.owner)?;
            if e.field(parent.object, "AttachParent")? != parent.attach_parent {
                return Err("source static vertex original parent attachment link changed".into());
            }
        }
        Ok(())
    }
    // Borrowed provider guards never dispatch GetOwner recursively. Complete
    // owner PE checks bracket the bulk operation; its guard checks original
    // identities and fresh hard links on each native callback boundary.
    fn profile_guard(&self, e: &impl Engine, handle: u64) -> Result<(), String> {
        self.base(e)?;
        let c = self
            .components
            .get(handle.checked_sub(1).ok_or("source scope handle")? as usize)
            .ok_or("source scope handle")?;
        e.verify(c.object)?;
        if !e.path_matches(c.object, &c.path, c.path_witness.as_ref())?
            || e.world(c.object)? != self.world.address
        {
            return Err("source spline original path/world changed".into());
        }
        let owner_check = |address| -> Result<(), String> {
            let o = self
                .owners
                .get(&address)
                .ok_or("source spline original owner")?;
            e.verify(o.object)?;
            e.verify(o.root)?;
            if e.world(o.object)? != self.world.address
                || e.world(o.root)? != self.world.address
                || e.field(o.object, "RootComponent")? != o.root.address
                || o.field
                    .as_ref()
                    .is_some_and(|f| e.field(self.pawn, f).ok() != Some(address))
            {
                return Err("source spline owner/root/world changed".into());
            }
            Ok(())
        };
        owner_check(c.owner)?;
        if let Some(p) = c.parent {
            e.verify(p.object)?;
            owner_check(p.owner)?;
            if e.world(p.object)? != self.world.address {
                return Err("source spline original parent world changed".into());
            }
        }
        if e.field(c.object, "AttachParent")? != c.parent.map_or(0, |p| p.object.address) {
            return Err("source spline original parent changed".into());
        }
        self.base(e)?;
        if !e.path_matches(c.object, &c.path, c.path_witness.as_ref())? {
            return Err("source spline hierarchy changed during native getters".into());
        }
        Ok(())
    }
    fn base(&self, e: &impl Engine) -> Result<(), String> {
        e.admit()?;
        for id in [self.world, self.pawn, self.controller] {
            e.verify(id)?;
        }
        if e.world(self.pawn)? != self.world.address
            || e.world(self.controller)? != self.world.address
            || e.field(self.pawn, "Controller")? != self.controller.address
            || e.field(self.controller, "Pawn")? != self.pawn.address
        {
            return Err("source scope pawn/controller/world changed".into());
        }
        e.admit()
    }
    fn qualify_owner(&self, e: &impl Engine, address: u64) -> Result<(), String> {
        let owner = self
            .owners
            .get(&address)
            .ok_or("source scope owner not captured")?;
        e.verify(owner.object)?;
        e.verify(owner.root)?;
        if e.world(owner.object)? != self.world.address
            || e.world(owner.root)? != self.world.address
            || e.owner(owner.root)? != address
            || e.field(owner.object, "RootComponent")? != owner.root.address
            || owner
                .field
                .as_ref()
                .is_some_and(|field| e.field(self.pawn, field).ok() != Some(address))
        {
            return Err("source scope original owner/root/weapon changed".into());
        }
        Ok(())
    }
    fn qualify_owners(&self, e: &impl Engine) -> Result<(), String> {
        for address in self.owners.keys() {
            self.qualify_owner(e, *address)?;
        }
        // The last owner's hard-link tail is pure. A directory change during
        // those reads must still refuse before begin stores/returns the scope.
        e.admit()
    }
    fn keep(
        &mut self,
        e: &impl Engine,
        address: u64,
        owner: u64,
        path: String,
    ) -> Result<u64, String> {
        self.base(e)?;
        if path.is_empty() || path.len() > 512 || path.contains('\0') || e.find(&path)? != address {
            return Err("source scope exact runtime path changed".into());
        }
        let object = e.capture(address)?;
        let path_witness = e.capture_path(object)?;
        // The lookup may invoke native name conversion. Tie the complete
        // captured hierarchy to the exact path again, then requalify it so a
        // rename/reparent during either lookup cannot bind a different path.
        if e.find(&path)? != address || !e.path_matches(object, &path, path_witness.as_ref())? {
            return Err("source scope initial path hierarchy changed".into());
        }
        e.verify(object)?;
        self.base(e)?;
        if e.world(object)? != self.world.address {
            return Err("source scope component world changed before owner getter".into());
        }
        let actual_owner = e.owner(object)?;
        let owner = if owner == 0 { actual_owner } else { owner };
        self.qualify_owner(e, owner)?;
        if e.world(object)? != self.world.address || actual_owner != owner {
            return Err("source scope component owner/world changed".into());
        }
        let parent_address = e.field(object, "AttachParent")?;
        let parent = if parent_address == 0 {
            None
        } else {
            let object = e.capture(parent_address)?;
            if e.world(object)? != self.world.address {
                return Err("source scope parent world changed".into());
            }
            let owner = e.owner(object)?;
            self.qualify_owner(e, owner)?;
            let attach_parent = e.field(object, "AttachParent")?;
            e.verify(object)?;
            Some(Parent {
                object,
                owner,
                attach_parent,
            })
        };
        if let Some(p) = parent {
            if e.world(p.object)? != self.world.address {
                return Err("source scope parent world changed".into());
            }
        }
        let next = Component {
            object,
            owner,
            parent,
            path,
            path_witness,
        };
        if let Some(i) = self
            .components
            .iter()
            .position(|c| c.object.address == address)
        {
            let c = &self.components[i];
            if c.object != next.object
                || c.owner != next.owner
                || c.parent != next.parent
                || c.path != next.path
                || c.path_witness != next.path_witness
            {
                return Err("source scope original identity reused".into());
            }
            self.resolve(e, i as u64 + 1)?;
            return Ok(i as u64 + 1);
        }
        if self.components.len() >= MAX_COMPONENTS {
            return Err("source scope component bound".into());
        }
        self.components.push(next);
        let handle = self.components.len() as u64;
        self.resolve(e, handle)?;
        Ok(handle)
    }
    fn resolve(&self, e: &impl Engine, handle: u64) -> Result<u64, String> {
        self.base(e)?;
        let c = self
            .components
            .get(handle.checked_sub(1).ok_or("source scope handle")? as usize)
            .ok_or("source scope handle")?;
        self.qualify_owner(e, c.owner)?;
        e.verify(c.object)?;
        if !e.path_matches(c.object, &c.path, c.path_witness.as_ref())?
            || e.world(c.object)? != self.world.address
            || e.owner(c.object)? != c.owner
        {
            return Err("source scope component path/owner/world changed".into());
        }
        if let Some(p) = c.parent {
            e.verify(p.object)?;
            self.qualify_owner(e, p.owner)?;
            if e.world(p.object)? != self.world.address || e.owner(p.object)? != p.owner {
                return Err("source scope original parent owner/world changed".into());
            }
        }
        if e.field(c.object, "AttachParent")? != c.parent.map_or(0, |p| p.object.address) {
            return Err("source scope hard parent link changed".into());
        }
        self.base(e)?;
        if !e.path_matches(c.object, &c.path, c.path_witness.as_ref())? {
            return Err("source scope hierarchy changed during native getters".into());
        }
        e.admit()?;
        Ok(c.object.address)
    }
    #[cfg(test)]
    fn resolve_object(
        &self,
        e: &impl Engine,
        handle: u64,
        factory: impl FnOnce(u64) -> Result<(), String>,
        finish: impl FnOnce() -> Result<(), String>,
    ) -> Result<u64, String> {
        self.resolve_return(e, handle, false, factory, finish)
    }
    fn resolve_return(
        &self,
        e: &impl Engine,
        handle: u64,
        address_only: bool,
        factory: impl FnOnce(u64) -> Result<(), String>,
        finish: impl FnOnce() -> Result<(), String>,
    ) -> Result<u64, String> {
        let address = self.resolve(e, handle)?;
        if !address_only {
            factory(address)?;
            // Factory allocation, UE4SS dispatch postcallbacks and any Lua GC
            // have completed. Never bless a rebound object after callbacks.
            if self.resolve(e, handle)? != address {
                return Err("source wrapper original object changed".into());
            }
        }
        // Scalar validation still needs the same original path/hard-link and
        // admission tail; it skips only construction and its postcallbacks.
        finish()?;
        Ok(address)
    }
}

struct Runtime<'a> {
    n: &'a Native,
    vt: &'a HsmpReflect,
    x: Exports,
    key: &'a [u8],
    base: Vec<Identity>,
    reference: EntityRef,
    dir_seq: u32,
    index: u8,
    schema: Rc<RefCell<SchemaCache>>,
}
// Scalar provider-guard reads stay pure; virtual world getters still use fresh
// Runtime admission on both sides. Every original identity/link/path is read
// freshly. This adapter cannot dispatch GetOwner or inspect a new schema.
struct ProfileGuardEngine<'a, 'b> {
    runtime: &'a Runtime<'b>,
}
impl Engine for ProfileGuardEngine<'_, '_> {
    fn capture(&self, _: u64) -> Result<Identity, String> {
        Err("source profile guard uncaptured schema identity".into())
    }
    fn verify(&self, id: Identity) -> Result<(), String> {
        self.runtime.identity(id).map(|_| ())
    }
    fn world(&self, id: Identity) -> Result<u64, String> {
        self.runtime.world(id)
    }
    fn field(&self, id: Identity, name: &str) -> Result<u64, String> {
        cached_field(&self.runtime.schema, self, id, name)
    }
    fn owner(&self, _: Identity) -> Result<u64, String> {
        Err("source profile guard GetOwner dispatch forbidden".into())
    }
    fn find(&self, _: &str) -> Result<u64, String> {
        Err("source profile guard global path lookup forbidden".into())
    }
    fn path_matches(
        &self,
        id: Identity,
        _: &str,
        witness: Option<&PathWitness>,
    ) -> Result<bool, String> {
        self.runtime.verify_path(id, witness)
    }
}
impl SchemaEngine for ProfileGuardEngine<'_, '_> {
    fn schema_name(&self, _: &str) -> Result<u64, String> {
        Err("source profile guard schema-name conversion forbidden".into())
    }
    fn property(&self, _: Identity, _: &str) -> Result<reflect::HsmpProp, String> {
        Err("source profile guard uncached property schema".into())
    }
    fn parameters(&self, _: Identity) -> Result<(Vec<reflect::HsmpProp>, i32), String> {
        Err("source profile guard function inspection forbidden".into())
    }
    fn read_pointer(&self, id: Identity, property: reflect::HsmpProp) -> Result<u64, String> {
        let p = self.runtime.identity(id)?;
        let value = unsafe {
            std::ptr::read_unaligned((p as *const u8).add(property.offset as usize).cast::<u64>())
        };
        self.runtime.identity(id)?;
        Ok(value)
    }
    fn dispatch_owner(&self, _: Identity, _: Identity, _: Identity) -> Result<u64, String> {
        Err("source profile guard ProcessEvent forbidden".into())
    }
}
impl Runtime<'_> {
    fn verify_path(&self, id: Identity, witness: Option<&PathWitness>) -> Result<bool, String> {
        self.identity(id)?;
        let witness = witness.ok_or("source original path witness missing")?;
        let first = witness
            .nodes
            .first()
            .ok_or("source original path witness empty")?;
        if witness.nodes.len() > 64
            || (
                first.weak,
                first.address,
                first.name,
                first.class_weak,
                first.class_address,
            ) != (
                id.weak,
                id.address,
                id.name,
                id.class_weak,
                id.class_address,
            )
        {
            return Err("source original path root identity changed".into());
        }
        let reader = path_reader()?;
        let mut reason = [0u8; 192];
        let mut package_name = witness.package_name;
        if unsafe {
            reader(
                witness.nodes.as_ptr(),
                witness.nodes.len() as u32,
                0,
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
                &mut package_name,
                reason.as_mut_ptr().cast(),
                192,
            )
        } != 1
        {
            return Err(path_error(&reason));
        }
        self.identity(id)?;
        Ok(true)
    }
    fn admit(&self) -> Result<(), String> {
        hsmp_native_profile_tick(1);
        if !self.n.native_host.is_host()
            || self.n.poisoned
            || self.n.game_thread != Some(std::thread::current().id())
            || self.n.world_key.as_deref() != Some(self.key)
        {
            return Err("source scope role/thread/world admission".into());
        }
        profile_once(1, c"source_directory", 0);
        let d = self
            .n
            .native_host
            .directory()
            .ok_or("source scope directory unavailable")?;
        profile_once(2, c"source_directory", 1);
        if d.epoch != self.reference.epoch
            || d.seq != self.dir_seq
            || !d
                .entities
                .iter()
                .any(|e| e.reference == self.reference && e.controller == self.index)
        {
            return Err("source scope entity generation changed".into());
        }
        // Before native_guard_ok's property reads, validate all originally admitted
        // native objects including RF_MirroredGarbage. No ProcessEvent here.
        for id in &self.base {
            self.identity(*id)?;
        }
        if self.base.len() == 3 && !self.n.native_guard_ok(self.vt) {
            return Err("source scope native world guard changed".into());
        }
        Ok(())
    }
    fn live(&self, weak: u64, address: u64) -> Result<*mut c_void, String> {
        unsafe {
            let p = (self.vt.resolve)(weak);
            if weak == 0 || p.is_null() || p as u64 != address {
                return Err("source scope original slot/serial/address changed".into());
            }
            let flags = (self.x.flags)(p);
            if flags.is_null() || *flags & GARBAGE != 0 {
                return Err("source scope original object garbage".into());
            }
            Ok(p)
        }
    }
    fn identity(&self, id: Identity) -> Result<*mut c_void, String> {
        unsafe {
            let p = self.live(id.weak, id.address)?;
            let cls = self.live(id.class_weak, id.class_address)?;
            let name = (self.x.name)(p);
            if name.is_null() || *name != id.name || (self.vt.class_of)(p) != cls {
                return Err("source scope original FName/class changed".into());
            }
            Ok(p)
        }
    }
}
impl Engine for Runtime<'_> {
    fn admit(&self) -> Result<(), String> {
        Runtime::admit(self)
    }
    fn capture(&self, address: u64) -> Result<Identity, String> {
        self.admit()?;
        unsafe {
            if address == 0 {
                return Err("source scope null object".into());
            }
            let weak = (self.vt.weak)(address as *mut c_void);
            let p = self.live(weak, address)?;
            let cls = (self.vt.class_of)(p);
            let class_weak = if cls.is_null() {
                0
            } else {
                (self.vt.weak)(cls)
            };
            self.live(class_weak, cls as u64)?;
            let name = (self.x.name)(p);
            if name.is_null() {
                return Err("source scope native FName unavailable".into());
            }
            let id = Identity {
                weak,
                address,
                name: *name,
                class_weak,
                class_address: cls as u64,
            };
            self.identity(id)?;
            self.admit()?;
            Ok(id)
        }
    }
    fn verify(&self, id: Identity) -> Result<(), String> {
        // Pure original slot/serial/address/full-FName/class/flags reads. The
        // surrounding operation and each callback still freshly admit; no
        // admission result/ticket survives a callback or is cached here.
        self.identity(id).map(|_| ())
    }
    fn world(&self, id: Identity) -> Result<u64, String> {
        self.admit()?;
        let p = self.identity(id)?;
        profile_once(4, c"source_world", 0);
        let result = unsafe { (self.x.world)(p) } as u64;
        profile_once(8, c"source_world", 1);
        self.identity(id)?;
        self.admit()?;
        Ok(result)
    }
    fn field(&self, id: Identity, name: &str) -> Result<u64, String> {
        cached_field(&self.schema, self, id, name)
    }
    fn owner(&self, id: Identity) -> Result<u64, String> {
        cached_owner(&self.schema, self, id)
    }
    fn find(&self, path: &str) -> Result<u64, String> {
        self.admit()?;
        hsmp_native_profile_tick(2);
        profile_once(16, c"source_find", 0);
        let p = unsafe { (self.vt.find)(reflect::wide(path).as_ptr()) } as u64;
        profile_once(32, c"source_find", 1);
        self.admit()?;
        Ok(p)
    }
    fn capture_path(&self, id: Identity) -> Result<Option<PathWitness>, String> {
        self.admit()?;
        self.identity(id)?;
        let class = self.live(id.class_weak, id.class_address)?;
        let class_name = unsafe { (self.x.name)(class) };
        if class_name.is_null() {
            return Err("source path original class FName unavailable".into());
        }
        let root = PathNode {
            weak: id.weak,
            address: id.address,
            name: id.name,
            class_weak: id.class_weak,
            class_address: id.class_address,
            class_name: unsafe { *class_name },
        };
        let mut output = [PathNode::default(); 64];
        let mut count = 0;
        let mut package_name = 0;
        let mut reason = [0u8; 192];
        let reader = path_reader()?;
        if unsafe {
            reader(
                &root,
                1,
                1,
                output.as_mut_ptr(),
                64,
                &mut count,
                &mut package_name,
                reason.as_mut_ptr().cast(),
                192,
            )
        } != 1
        {
            return Err(path_error(&reason));
        }
        if count == 0 || count > 64 || output[0] != root {
            return Err("source original path witness bounds/root".into());
        }
        self.identity(id)?;
        self.admit()?;
        Ok(Some(PathWitness {
            nodes: output[..count as usize].to_vec(),
            package_name,
        }))
    }
    fn path_matches(
        &self,
        id: Identity,
        _: &str,
        witness: Option<&PathWitness>,
    ) -> Result<bool, String> {
        // The pinned reader copies/verifies the entire original Outer chain
        // and GPackageName without PE/name conversion or engine callbacks.
        self.verify_path(id, witness)
    }
}
impl SchemaEngine for Runtime<'_> {
    fn schema_name(&self, name: &str) -> Result<u64, String> {
        self.admit()?;
        let value = unsafe { (self.vt.fname)(reflect::wide(name).as_ptr(), 1) };
        self.admit()?;
        Ok(value)
    }
    fn property(&self, id: Identity, name: &str) -> Result<reflect::HsmpProp, String> {
        self.admit()?;
        let p = self.identity(id)?;
        unsafe {
            let mut prop = reflect::HsmpProp::default();
            if (self.vt.obj_prop)(p, reflect::wide(name).as_ptr(), &mut prop) != 1 {
                return Err(format!("source scope hard {name} property ABI"));
            }
            self.identity(id)?;
            self.admit()?;
            Ok(prop)
        }
    }
    fn parameters(&self, function: Identity) -> Result<(Vec<reflect::HsmpProp>, i32), String> {
        self.admit()?;
        let result = unsafe { crate::sample::props_of(self.vt, self.identity(function)?) };
        self.identity(function)?;
        self.admit()?;
        Ok(result)
    }
    fn read_pointer(&self, id: Identity, prop: reflect::HsmpProp) -> Result<u64, String> {
        let p = self.identity(id)?;
        unsafe {
            let value =
                std::ptr::read_unaligned((p as *const u8).add(prop.offset as usize).cast::<u64>());
            self.identity(id)?;
            Ok(value)
        }
    }
    fn dispatch_owner(
        &self,
        id: Identity,
        function: Identity,
        class: Identity,
    ) -> Result<u64, String> {
        self.admit()?;
        unsafe {
            let p = self.identity(id)?;
            if (self.vt.is_a)(p, self.identity(class)?) != 1 {
                return Err("source scope component class".into());
            }
            let mut result = 0u64;
            self.admit()?;
            let function_pointer = self.identity(function)?;
            self.identity(class)?;
            let p = self.identity(id)?;
            hsmp_native_profile_tick(3);
            profile_checkpoint(c"scope_owner_pe", 0);
            (self.vt.call)(p, function_pointer, (&mut result as *mut u64).cast());
            profile_checkpoint(c"scope_owner_pe", 1);
            self.identity(id)?;
            self.identity(function)?;
            self.identity(class)?;
            self.admit()?;
            Ok(result)
        }
    }
}
#[derive(Default)]
struct State {
    seq: u64,
    scope: Option<Scope>,
}
thread_local! { static STATE: RefCell<State> = RefCell::new(State::default()); }
unsafe fn integer(L: *mut lua_State, t: c_int, key: &str) -> Result<i64, String> {
    unsafe {
        rawget_str(L, t, key);
        let v = arg_int(L, -1);
        pop(L, 1);
        v.ok_or_else(|| format!("source scope {key}"))
    }
}
unsafe fn string(L: *mut lua_State, t: c_int, key: &str) -> Result<String, String> {
    unsafe {
        rawget_str(L, t, key);
        let v = arg_str(L, -1).map(str::to_owned);
        pop(L, 1);
        v.ok_or_else(|| format!("source scope {key}"))
    }
}
fn runtime<'a>(n: &'a Native, vt: &'a HsmpReflect, x: Exports, s: &'a Scope) -> Runtime<'a> {
    Runtime {
        n,
        vt,
        x,
        key: &s.key,
        base: vec![s.world, s.pawn, s.controller],
        reference: s.reference,
        dir_seq: s.dir_seq,
        index: s.index,
        schema: s.schema.clone(),
    }
}
fn reference(epoch: i64, id: i64, incarnation: i64) -> Result<EntityRef, String> {
    let r = EntityRef {
        epoch: epoch as u64,
        id: u32::try_from(id).map_err(|_| "source scope id")?,
        incarnation: u32::try_from(incarnation).map_err(|_| "source scope incarnation")?,
    };
    if r.valid() {
        Ok(r)
    } else {
        Err("source scope reference".into())
    }
}
struct SplineScopeGuard<'a> {
    scope: &'a Scope,
    engine: Runtime<'a>,
    handle: u64,
}
impl SplineScopeGuard<'_> {
    fn valid(&self) -> Result<(), String> {
        let e = &self.engine;
        e.admit()?;
        self.scope
            .profile_guard(&ProfileGuardEngine { runtime: e }, self.handle)?;
        e.admit()
    }
}
unsafe extern "C" fn spline_scope_guard(context: *mut c_void) -> i32 {
    if context.is_null() {
        return 0;
    }
    let c = unsafe { &*(context as *const SplineScopeGuard<'_>) };
    i32::from(c.valid().is_ok())
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct StaticVertexState {
    lod_info_count: u32,
    no_override: bool,
    asset_present: bool,
    material_count: u32,
    material_null_mask: u32,
    component_kind: u32,
}
impl StaticVertexState {
    fn state(self) -> &'static str {
        if !self.asset_present {
            if self.component_kind == 0 {
                "native_empty_skeletal"
            } else {
                "native_empty"
            }
        } else if self.no_override {
            "native_asset"
        } else {
            "captured_required"
        }
    }
}
fn checked_vertex_state(
    proof: crate::native_presentation::VertexStateProof,
) -> Result<StaticVertexState, String> {
    if proof.lod_info_count > 16
        || proof.no_override > 1
        || proof.asset_present > 1
        || proof.component_kind > 1
        || proof.material_count > 32
        || (proof.material_count < 32 && (proof.material_null_mask >> proof.material_count) != 0)
        || (proof.component_kind == 0 && proof.asset_present != 0)
        || (proof.asset_present == 0 && proof.no_override == 0)
        || (proof.no_override == 0 && proof.lod_info_count == 0)
    {
        return Err("source static vertex proof bounds/state incomplete".into());
    }
    Ok(StaticVertexState {
        lod_info_count: proof.lod_info_count,
        no_override: proof.no_override == 1,
        asset_present: proof.asset_present == 1,
        material_count: proof.material_count,
        material_null_mask: proof.material_null_mask,
        component_kind: proof.component_kind,
    })
}
fn capture_static_vertex_state(
    scope: &Scope,
    engine: &impl Engine,
    handle: u64,
    mut describe: impl FnMut() -> Result<crate::native_presentation::VertexStateProof, String>,
    final_check: impl FnOnce() -> Result<(), String>,
) -> Result<StaticVertexState, String> {
    scope.resolve(engine, handle)?;
    let first = checked_vertex_state(describe()?)?;
    // GetOwner/GetWorld in the full original resolve can reenter native code.
    // A second provider census must follow it; never reuse the earlier proof.
    scope.resolve(engine, handle)?;
    let last = checked_vertex_state(describe()?)?;
    if last != first {
        return Err("source static vertex state changed during native getters".into());
    }
    final_check()?;
    Ok(last)
}
unsafe fn resolve_address_only(L: *mut lua_State) -> Result<bool, String> {
    unsafe {
        match lua_gettop(L) {
            2 => Ok(false),
            3 if lua_type(L, 3) == LUA_TBOOLEAN => Ok(lua_toboolean(L, 3) != 0),
            _ => Err("source scope resolve mode must be an optional boolean".into()),
        }
    }
}

impl Native {
    pub unsafe fn source_scope_vertex_state(&mut self, L: *mut lua_State) -> c_int {
        let result: Result<StaticVertexState, String> = (|| unsafe {
            let id = arg_int(L, 1).ok_or("source scope id")? as u64;
            let handle = arg_int(L, 2).ok_or("source scope handle")? as u64;
            let scope = STATE
                .with(|s| s.borrow_mut().scope.take())
                .ok_or("source scope unavailable")?;
            if scope.id != id {
                return Err("source scope id changed".into());
            }
            let row = scope
                .components
                .get(handle.checked_sub(1).ok_or("source scope handle")? as usize)
                .ok_or("source scope handle")?;
            let owner = scope
                .owners
                .get(&row.owner)
                .ok_or("source scope owner")?
                .object;
            let object = |i: Identity| crate::native_presentation::Object {
                weak: i.weak,
                address: i.address,
            };
            let vt = reflect::vt().ok_or("source scope reflection")?;
            let context = SplineScopeGuard {
                scope: &scope,
                engine: runtime(self, vt, exports()?, &scope),
                handle,
            };
            let guard = crate::native_presentation::Guard {
                context: (&context as *const SplineScopeGuard<'_>).cast_mut().cast(),
                check: spline_scope_guard,
            };
            let provider = crate::native_presentation::provider()?;
            let proof = capture_static_vertex_state(
                &scope,
                &context.engine,
                handle,
                || {
                    context.valid()?;
                    let mut proof = crate::native_presentation::VertexStateProof::default();
                    let mut info = crate::native_presentation::ResultInfo::default();
                    if (provider.describe_vertex_state)(
                        object(scope.world),
                        object(owner),
                        object(row.object),
                        &guard,
                        &mut proof,
                        &mut info,
                    ) != 1
                        || info.complete != 1
                    {
                        return Err(format!("source static vertex proof: {}", info.reason()));
                    }
                    Ok(proof)
                },
                || {
                    // The final provider census follows every callback-capable
                    // getter. No full resolve/guard runs after this pure region.
                    context.engine.admit()?;
                    context
                        .engine
                        .verify_path(row.object, row.path_witness.as_ref())?;
                    scope.pure_hard_links(
                        &ProfileGuardEngine {
                            runtime: &context.engine,
                        },
                        handle,
                    )?;
                    context
                        .engine
                        .verify_path(row.object, row.path_witness.as_ref())?;
                    context.engine.admit()
                },
            )?;
            drop(context);
            STATE.with(|s| {
                let mut state = s.borrow_mut();
                if state.scope.is_some() || state.seq != id {
                    return Err("source scope reentry changed".into());
                }
                state.scope = Some(scope);
                Ok::<_, String>(())
            })?;
            Ok(proof)
        })();
        unsafe {
            match result {
                Ok(proof) => {
                    lua_createtable(L, 0, 7);
                    let t = lua_gettop(L);
                    set_str(L, t, "state", proof.state());
                    set_int(L, t, "lod_info_count", proof.lod_info_count.into());
                    set_bool(L, t, "no_override", proof.no_override);
                    set_bool(L, t, "asset_present", proof.asset_present);
                    set_int(L, t, "material_count", proof.material_count.into());
                    set_int(L, t, "material_null_mask", proof.material_null_mask.into());
                    set_str(
                        L,
                        t,
                        "component_kind",
                        if proof.component_kind == 0 {
                            "skeletal"
                        } else {
                            "static"
                        },
                    );
                    1
                }
                Err(reason) => nil_err(L, &reason),
            }
        }
    }
    pub unsafe fn source_scope_spline_profile(&mut self, L: *mut lua_State) -> c_int {
        let result = (|| unsafe {
            let id = arg_int(L, 1).ok_or("source scope id")? as u64;
            let handle = arg_int(L, 2).ok_or("source scope handle")? as u64;
            let scope = STATE
                .with(|s| s.borrow_mut().scope.take())
                .ok_or("source scope unavailable")?;
            if scope.id != id {
                return Err("source scope id changed".into());
            }
            let _profile = ProfileOperation::begin(scope.reference, scope.dir_seq, handle);
            let vt = reflect::vt().ok_or("source scope reflection")?;
            let engine = runtime(self, vt, exports()?, &scope);
            profile_checkpoint(c"scope_initial_resolve", 0);
            scope.resolve(&engine, handle)?;
            profile_checkpoint(c"scope_initial_resolve", 1);
            let row = scope
                .components
                .get(handle.checked_sub(1).ok_or("source scope handle")? as usize)
                .ok_or("source scope handle")?;
            let owner = scope
                .owners
                .get(&row.owner)
                .ok_or("source scope owner")?
                .object;
            let object = |i: Identity| crate::native_presentation::Object {
                weak: i.weak,
                address: i.address,
            };
            let mut context = SplineScopeGuard {
                scope: &scope,
                engine,
                handle,
            };
            profile_checkpoint(c"scope_guard", 0);
            context.valid()?;
            profile_checkpoint(c"scope_guard", 1);
            let guard = crate::native_presentation::Guard {
                context: (&mut context as *mut SplineScopeGuard<'_>).cast(),
                check: spline_scope_guard,
            };
            let mut profile = crate::native_presentation::SplineProfile::default();
            let mut info = crate::native_presentation::ResultInfo::default();
            let p = crate::native_presentation::provider()?;
            profile_checkpoint(c"provider", 0);
            if (p.describe_spline)(
                object(scope.world),
                object(owner),
                object(row.object),
                &guard,
                &mut profile,
                &mut info,
            ) != 1
                || info.complete != 1
            {
                return Err(format!("source spline profile: {}", info.reason()));
            }
            profile_checkpoint(c"provider", 1);
            context.valid()?;
            profile_checkpoint(c"scope_final_resolve", 0);
            scope.resolve(&context.engine, handle)?;
            profile_checkpoint(c"scope_final_resolve", 1);
            if profile.metadata_null != 1
                || profile.position_count > 64
                || profile.rotation_count > 64
                || profile.scale_count > 64
                || profile.reparam_count > 1024
            {
                return Err("source spline profile bounds/metadata".into());
            }
            drop(context);
            STATE.with(|s| {
                let mut state = s.borrow_mut();
                if state.scope.is_some() || state.seq != id {
                    return Err("source scope reentry changed".into());
                }
                state.scope = Some(scope);
                Ok::<_, String>(())
            })?;
            Ok(profile)
        })();
        unsafe {
            match result {
                Ok(p) => {
                    lua_createtable(L, 0, 5);
                    let t = lua_gettop(L);
                    set_int(L, t, "position_count", p.position_count.into());
                    set_int(L, t, "rotation_count", p.rotation_count.into());
                    set_int(L, t, "scale_count", p.scale_count.into());
                    set_int(L, t, "reparam_count", p.reparam_count.into());
                    set_bool(L, t, "metadata_null", true);
                    1
                }
                Err(e) => nil_err(L, &e),
            }
        }
    }
    pub unsafe fn source_scope_begin(&mut self, L: *mut lua_State) -> c_int {
        let result = (|| unsafe {
            if !is_table(L, 1) || !is_table(L, 2) {
                return Err("source scope begin tables".into());
            }
            let reference = reference(
                integer(L, 1, "epoch")?,
                integer(L, 1, "id")?,
                integer(L, 1, "incarnation")?,
            )?;
            let dir_seq = u32::try_from(integer(L, 1, "dir_seq")?)
                .map_err(|_| "source scope directory seq")?;
            let index = u8::try_from(integer(L, 2, "index")?).map_err(|_| "source scope index")?;
            let key = self.world_key.clone().ok_or("source scope world token")?;
            let vt = reflect::vt().ok_or("source scope reflection")?;
            let x = exports()?;
            let mut e = Runtime {
                n: self,
                vt,
                x,
                key: &key,
                base: vec![],
                reference,
                dir_seq,
                index,
                schema: Rc::new(RefCell::new(SchemaCache::default())),
            };
            // Bindings were freshly resolved in Lua. Original identities are
            // captured before world/owner getters and admitted again afterward.
            let world = e.capture(integer(L, 2, "world")? as u64)?;
            e.base.push(world);
            let pawn = e.capture(integer(L, 2, "pawn")? as u64)?;
            e.base.push(pawn);
            let controller = e.capture(integer(L, 2, "controller")? as u64)?;
            e.base.push(controller);
            let mut s = Scope {
                id: 0,
                reference,
                dir_seq,
                index,
                key: key.clone(),
                world,
                pawn,
                controller,
                owners: HashMap::new(),
                components: vec![],
                schema: e.schema.clone(),
            };
            s.base(&e)?;
            let root = e.capture(e.field(pawn, "RootComponent")?)?;
            s.owners.insert(
                pawn.address,
                Owner {
                    object: pawn,
                    root,
                    field: None,
                },
            );
            rawget_str(L, 2, "weapons");
            if !is_table(L, -1) {
                pop(L, 1);
                return Err("source scope weapons".into());
            }
            let rows = lua_absindex(L, -1);
            let len = lua_rawlen(L, rows);
            if len > 7 {
                pop(L, 1);
                return Err("source scope weapon bound".into());
            }
            for i in 1..=len {
                lua_rawgeti(L, rows, i as i64);
                if !is_table(L, -1) {
                    pop(L, 2);
                    return Err("source scope weapon binding".into());
                }
                let t = lua_absindex(L, -1);
                let address = integer(L, t, "address")? as u64;
                let field = string(L, t, "field")?;
                pop(L, 1);
                if ![
                    "Weapon R",
                    "Weapon L",
                    "Weapon Slot R 1",
                    "Weapon Slot R 2",
                    "Weapon Slot Back",
                    "Weapon Slot L 1",
                    "Weapon Slot L 2",
                ]
                .contains(&field.as_str())
                    || e.field(pawn, &field)? != address
                {
                    pop(L, 1);
                    return Err("source scope original weapon field".into());
                }
                let object = e.capture(address)?;
                let root = e.capture(e.field(object, "RootComponent")?)?;
                if s.owners
                    .insert(
                        address,
                        Owner {
                            object,
                            root,
                            field: Some(field),
                        },
                    )
                    .is_some()
                {
                    pop(L, 1);
                    return Err("source scope duplicate owner".into());
                }
            }
            pop(L, 1);
            s.qualify_owners(&e)?;
            STATE.with(|state| {
                let mut state = state.borrow_mut();
                state.seq = state
                    .seq
                    .checked_add(1)
                    .ok_or("source scope sequence exhausted")?;
                s.id = state.seq;
                let id = s.id;
                state.scope = Some(s);
                Ok::<_, String>(id)
            })
        })();
        unsafe {
            match result {
                Ok(id) => {
                    lua_pushinteger(L, id as i64);
                    1
                }
                Err(reason) => {
                    STATE.with(|s| s.borrow_mut().scope = None);
                    nil_err(L, &reason)
                }
            }
        }
    }
    pub unsafe fn source_scope_keep(&mut self, L: *mut lua_State) -> c_int {
        unsafe { self.source_scope_op(L, true) }
    }
    pub unsafe fn source_scope_resolve(&mut self, L: *mut lua_State) -> c_int {
        let top = unsafe { lua_gettop(L) };
        let result = (|| unsafe {
            let id = arg_int(L, 1).ok_or("source scope id")? as u64;
            let handle = arg_int(L, 2).ok_or("source scope handle")? as u64;
            let address_only = resolve_address_only(L)?;
            // Release STATE before any engine getter or protected Lua call.
            // A reentrant begin/end cannot borrow through this operation or
            // overwrite its original scope; the sequence is checked at restore.
            let scope = STATE
                .with(|s| s.borrow_mut().scope.take())
                .ok_or("source scope unavailable")?;
            if scope.id != id {
                return Err("source scope id changed".into());
            }
            let vt = reflect::vt().ok_or("source scope reflection")?;
            let e = runtime(self, vt, exports()?, &scope);
            e.admit()?;
            let address = scope.resolve_return(
                &e,
                handle,
                address_only,
                |address| {
                    let factory = object_factory()?;
                    // Scalar push is nonallocating. The C++ protected factory
                    // adds the fresh userdata after it, preserving return order.
                    lua_pushinteger(L, address as i64);
                    let before = lua_gettop(L);
                    let mut reason = [0u8; 192];
                    e.admit()?;
                    if factory(L, address, reason.as_mut_ptr().cast(), 192) != 1 {
                        let n = reason.iter().position(|c| *c == 0).unwrap_or(reason.len());
                        return Err(format!(
                            "source wrapper: {}",
                            String::from_utf8_lossy(&reason[..n])
                        ));
                    }
                    e.admit()?;
                    // LUA_TUSERDATA=7 in pinned Lua 5.4; table/light userdata
                    // must never stand in for a fresh UE4SS remote UObject.
                    if lua_gettop(L) != before + 1 || lua_type(L, -1) != 7 {
                        return Err("source wrapper userdata/stack contract".into());
                    }
                    Ok(())
                },
                || {
                    let row = scope
                        .components
                        .get(handle.checked_sub(1).ok_or("source scope handle")? as usize)
                        .ok_or("source scope handle")?;
                    // Callback-free closure after the last world/owner getter.
                    // It retains role/ref/dir, original slot/name/class/flags,
                    // complete native path and every original cached hard link.
                    e.admit()?;
                    e.verify_path(row.object, row.path_witness.as_ref())?;
                    scope.pure_hard_links(&ProfileGuardEngine { runtime: &e }, handle)?;
                    e.verify_path(row.object, row.path_witness.as_ref())?;
                    e.admit()
                },
            )?;
            drop(e);
            STATE.with(|s| {
                let mut state = s.borrow_mut();
                if state.scope.is_some() || state.seq != id {
                    state.scope = None;
                    return Err("source scope reentry changed".into());
                }
                state.scope = Some(scope);
                Ok::<_, String>(())
            })?;
            if address_only {
                lua_pushinteger(L, address as i64);
            }
            Ok::<_, String>(if address_only { 1 } else { 2 })
        })();
        unsafe {
            match result {
                Ok(count) => count,
                Err(reason) => {
                    STATE.with(|s| s.borrow_mut().scope = None);
                    lua_settop(L, top);
                    nil_err(L, &reason)
                }
            }
        }
    }
    unsafe fn source_scope_op(&mut self, L: *mut lua_State, keep: bool) -> c_int {
        let result = (|| unsafe {
            let id = arg_int(L, 1).ok_or("source scope id")? as u64;
            let vt = reflect::vt().ok_or("source scope reflection")?;
            let x = exports()?;
            STATE.with(|state| {
                let mut state = state.borrow_mut();
                let mut s = state.scope.take().ok_or("source scope unavailable")?;
                if s.id != id {
                    return Err("source scope id changed".into());
                }
                let e = runtime(self, vt, x, &s);
                e.admit()?;
                let result = if keep {
                    if !is_table(L, 2) {
                        Err("source scope keep table".into())
                    } else {
                        let address = integer(L, 2, "address")? as u64;
                        let owner = integer(L, 2, "owner")? as u64;
                        let path = string(L, 2, "path")?;
                        // e borrows scalar scope metadata only; clone that small
                        // admission context before mutating component storage.
                        let key = s.key.clone();
                        let e = Runtime {
                            n: self,
                            vt,
                            x,
                            key: &key,
                            base: e.base.clone(),
                            reference: s.reference,
                            dir_seq: s.dir_seq,
                            index: s.index,
                            schema: s.schema.clone(),
                        };
                        s.keep(&e, address, owner, path)
                    }
                } else {
                    s.resolve(&e, arg_int(L, 2).ok_or("source scope handle")? as u64)
                };
                let result = result.map(|value| {
                    let owner = if keep {
                        Some(s.components[value as usize - 1].owner)
                    } else {
                        None
                    };
                    (value, owner)
                });
                if result.is_ok() {
                    state.scope = Some(s);
                }
                result
            })
        })();
        unsafe {
            match result {
                Ok((value, owner)) => {
                    lua_pushinteger(L, value as i64);
                    if let Some(owner) = owner {
                        lua_pushinteger(L, owner as i64);
                        2
                    } else {
                        1
                    }
                }
                Err(reason) => nil_err(L, &reason),
            }
        }
    }
    pub unsafe fn source_scope_end(&mut self, L: *mut lua_State) -> c_int {
        if !self.native_host.is_host()
            || self.poisoned
            || self.game_thread != Some(std::thread::current().id())
        {
            return unsafe { nil_err(L, "source scope end role/thread admission") };
        }
        let id = unsafe { arg_int(L, 1) };
        let ok = STATE.with(|s| {
            let mut s = s.borrow_mut();
            let ok = s
                .scope
                .as_ref()
                .is_some_and(|scope| Some(scope.id as i64) == id);
            s.scope = None;
            ok
        });
        unsafe {
            if ok {
                lua_pushboolean(L, 1);
                1
            } else {
                nil_err(L, "source scope id unavailable")
            }
        }
    }
}

#[cfg(test)]
mod source_scope_tests {
    use super::*;
    thread_local! {static TEST_PROFILE_LOG:RefCell<Vec<(String,u32,ProfileTrace)>>=const{RefCell::new(Vec::new())};}
    unsafe extern "C" fn test_profile_logger(
        stage: *const std::ffi::c_char,
        edge: u32,
        trace: *const ProfileTrace,
    ) {
        let stage = unsafe { std::ffi::CStr::from_ptr(stage) }
            .to_string_lossy()
            .into_owned();
        let trace = unsafe { *trace };
        TEST_PROFILE_LOG.with(|p| p.borrow_mut().push((stage, edge, trace)));
        // A callback can read/update TLS after the snapshot borrow has ended.
        hsmp_native_profile_tick(0);
    }
    #[test]
    fn source_scope_profile_trace_is_bounded_rare_and_callback_safe() {
        assert_eq!(std::mem::size_of::<ProfileTrace>(), 72);
        TEST_PROFILE_LOG.with(|p| p.borrow_mut().clear());
        hsmp_native_set_profile_logger(Some(test_profile_logger));
        profile_checkpoint(c"inactive", 0);
        hsmp_native_profile_tick(1);
        assert_eq!(TEST_PROFILE_LOG.with(|p| p.borrow().len()), 0);
        {
            let _scope = ProfileOperation::begin(
                EntityRef {
                    epoch: 0x800000000000000b,
                    id: 7,
                    incarnation: 9,
                },
                11,
                13,
            );
            for kind in 0..4 {
                hsmp_native_profile_tick(kind);
            }
            profile_checkpoint(c"counts", 1);
            let count = TEST_PROFILE_LOG.with(|p| p.borrow().len());
            profile_checkpoint(c"bad\nstage", 0);
            profile_checkpoint(c"bad_edge", 3);
            assert_eq!(TEST_PROFILE_LOG.with(|p| p.borrow().len()), count);
            for _ in 0..50000 {
                hsmp_native_profile_tick(1);
            }
            for _ in 0..100 {
                profile_checkpoint(c"checkpoint", 0);
            }
            TEST_PROFILE_LOG.with(|p| {
                let p = p.borrow();
                assert_eq!(p.len(), 64);
                assert_eq!(p[63].0, "diagnostic_budget_exhausted");
                assert_eq!(p[63].1, 2);
                assert_eq!(
                    p.iter()
                        .filter(|(s, _, _)| s == "diagnostic_budget_exhausted")
                        .count(),
                    1
                );
                assert_eq!(
                    p.iter().filter(|(s, _, _)| s == "guard_progress").count(),
                    8
                );
                for (i, (_, _, t)) in p.iter().enumerate() {
                    assert_eq!(t.seq, i as u32 + 1);
                    assert_eq!(
                        (t.epoch, t.entity, t.incarnation, t.dir_seq, t.handle),
                        (0x800000000000000b, 7, 9, 11, 13)
                    );
                    if i > 0 {
                        assert!(t.elapsed_us >= p[i - 1].2.elapsed_us);
                    }
                }
                assert_eq!((p[1].2.admissions, p[1].2.finds, p[1].2.events), (1, 1, 1));
            });
        }
        for _ in 0..100 {
            hsmp_native_profile_tick(1);
            profile_checkpoint(c"inactive_after", 0);
        }
        assert_eq!(TEST_PROFILE_LOG.with(|p| p.borrow().len()), 64);
        {
            let _scope = ProfileOperation::begin(
                EntityRef {
                    epoch: 1,
                    id: 2,
                    incarnation: 3,
                },
                4,
                5,
            );
        }
        TEST_PROFILE_LOG.with(|p| {
            let p = p.borrow();
            assert_eq!(p.len(), 66);
            assert_eq!((p[64].2.seq, p[65].2.seq), (1, 2));
            assert_eq!(p[65].0, "profile_operation");
            assert_eq!(p[65].1, 1);
        });
        hsmp_native_set_profile_logger(None);
    }
    #[derive(Clone)]
    struct Row {
        id: Identity,
        world: u64,
        owner: u64,
        root: u64,
        parent: u64,
        path: String,
        flags: u32,
    }
    struct Mock {
        rows: RefCell<HashMap<u64, Row>>,
        owner_calls: RefCell<u32>,
        find_calls: std::cell::Cell<u32>,
        rename_on_find: std::cell::Cell<u32>,
        world_calls: std::cell::Cell<u32>,
        rename_on_world: std::cell::Cell<u32>,
        vertex_override: std::cell::Cell<bool>,
        override_on_owner: std::cell::Cell<u32>,
        admissions: std::cell::Cell<u32>,
        generation_changed: std::cell::Cell<bool>,
        generation_on_world: std::cell::Cell<u32>,
        generation_on_owner: std::cell::Cell<u32>,
        field_calls: std::cell::Cell<u32>,
        generation_on_field: std::cell::Cell<u32>,
    }
    impl Mock {
        fn row(&self, id: Identity) -> Result<Row, String> {
            let row = self
                .rows
                .borrow()
                .get(&id.address)
                .cloned()
                .ok_or("fixture slot removed")?;
            if row.flags & GARBAGE != 0 {
                return Err("fixture garbage before engine getter".into());
            }
            if row.id != id {
                return Err("fixture original slot/name/class reused".into());
            }
            Ok(row)
        }
        fn change(&self, address: u64, f: impl FnOnce(&mut Row)) {
            f(self.rows.borrow_mut().get_mut(&address).unwrap());
        }
    }
    impl Engine for Mock {
        fn admit(&self) -> Result<(), String> {
            self.admissions.set(self.admissions.get() + 1);
            if self.generation_changed.get() {
                Err("fixture directory generation changed".into())
            } else {
                Ok(())
            }
        }
        fn capture(&self, address: u64) -> Result<Identity, String> {
            let id = self
                .rows
                .borrow()
                .get(&address)
                .ok_or("fixture missing")?
                .id;
            self.verify(id)?;
            Ok(id)
        }
        fn verify(&self, id: Identity) -> Result<(), String> {
            self.row(id).map(|_| ())
        }
        fn world(&self, id: Identity) -> Result<u64, String> {
            self.admit()?;
            let result = self.row(id)?.world;
            self.world_calls.set(self.world_calls.get() + 1);
            if self.rename_on_world.get() == self.world_calls.get() {
                self.change(7, |r| r.path.push_str("_renamed_during_world_getter"));
            }
            if self.generation_on_world.get() == self.world_calls.get() {
                self.generation_changed.set(true);
            }
            self.admit()?;
            Ok(result)
        }
        fn field(&self, id: Identity, name: &str) -> Result<u64, String> {
            let r = self.row(id)?;
            self.field_calls.set(self.field_calls.get() + 1);
            if self.generation_on_field.get() == self.field_calls.get() {
                self.generation_changed.set(true);
            }
            match name {
                "Controller" => Ok(3),
                "Pawn" => Ok(2),
                "RootComponent" => Ok(r.root),
                "AttachParent" => Ok(r.parent),
                "Weapon R" => Ok(5),
                _ => Err("fixture unknown field".into()),
            }
        }
        fn owner(&self, id: Identity) -> Result<u64, String> {
            self.admit()?;
            let r = self.row(id)?;
            *self.owner_calls.borrow_mut() += 1;
            if *self.owner_calls.borrow() == self.override_on_owner.get() {
                self.vertex_override.set(!self.vertex_override.get());
            }
            if self.generation_on_owner.get() == *self.owner_calls.borrow() {
                self.generation_changed.set(true);
            }
            self.admit()?;
            Ok(r.owner)
        }
        fn find(&self, path: &str) -> Result<u64, String> {
            let result = self
                .rows
                .borrow()
                .values()
                .find(|r| r.path == path)
                .map_or(0, |r| r.id.address);
            self.find_calls.set(self.find_calls.get() + 1);
            if self.rename_on_find.get() == self.find_calls.get() {
                self.change(7, |r| r.path.push_str("_renamed_during_lookup"));
            }
            Ok(result)
        }
    }
    fn fixture() -> (Scope, Mock) {
        let mut rows = HashMap::new();
        for address in 1..=7 {
            let id = Identity {
                weak: address,
                address,
                name: 100 + address,
                class_weak: 30,
                class_address: 30,
            };
            rows.insert(
                address,
                Row {
                    id,
                    world: 1,
                    owner: if address == 6 || address == 7 { 5 } else { 2 },
                    root: if address == 5 { 6 } else { 4 },
                    parent: if address == 7 { 4 } else { 0 },
                    path: format!("/Game/Arena.Arena:PersistentLevel.Actor{address}.Component"),
                    flags: 0,
                },
            );
        }
        let mock = Mock {
            rows: RefCell::new(rows),
            owner_calls: RefCell::new(0),
            find_calls: std::cell::Cell::new(0),
            rename_on_find: std::cell::Cell::new(0),
            world_calls: std::cell::Cell::new(0),
            rename_on_world: std::cell::Cell::new(0),
            vertex_override: std::cell::Cell::new(false),
            override_on_owner: std::cell::Cell::new(0),
            admissions: std::cell::Cell::new(0),
            generation_changed: std::cell::Cell::new(false),
            generation_on_world: std::cell::Cell::new(0),
            generation_on_owner: std::cell::Cell::new(0),
            field_calls: std::cell::Cell::new(0),
            generation_on_field: std::cell::Cell::new(0),
        };
        let s = Scope {
            id: 1,
            reference: reference(-7, 1, 2).unwrap(),
            dir_seq: 3,
            index: 0,
            key: b"world".to_vec(),
            world: mock.capture(1).unwrap(),
            pawn: mock.capture(2).unwrap(),
            controller: mock.capture(3).unwrap(),
            owners: HashMap::from([
                (
                    2,
                    Owner {
                        object: mock.capture(2).unwrap(),
                        root: mock.capture(4).unwrap(),
                        field: None,
                    },
                ),
                (
                    5,
                    Owner {
                        object: mock.capture(5).unwrap(),
                        root: mock.capture(6).unwrap(),
                        field: Some("Weapon R".into()),
                    },
                ),
            ]),
            components: vec![],
            schema: Rc::new(RefCell::new(SchemaCache::default())),
        };
        (s, mock)
    }
    #[test]
    fn source_scope_signed_epoch_preserves_original_bits() {
        assert_eq!(
            reference(i64::MIN + 11, 7, 13).unwrap().epoch,
            0x800000000000000b
        );
        assert!(reference(0, 7, 13).is_err());
        assert!(reference(-1, -1, 1).is_err());
    }
    #[test]
    fn source_scope_complete_original_owner_and_parent_roundtrip() {
        let (mut s, e) = fixture();
        let path = e.rows.borrow()[&7].path.clone();
        let h = s.keep(&e, 7, 0, path).unwrap();
        assert_eq!(s.components[h as usize - 1].owner, 5);
        assert_eq!(s.resolve(&e, h).unwrap(), 7);
        assert_eq!(s.components[0].parent.unwrap().object.address, 4);
    }
    #[test]
    fn source_scope_address_only_mode_is_explicit_and_does_not_change_lua_stack() {
        unsafe {
            let L: *mut lua_State = mlua::ffi::luaL_newstate().cast();
            assert!(!L.is_null());
            lua_pushinteger(L, 1);
            lua_pushinteger(L, 2);
            assert!(!resolve_address_only(L).unwrap());
            for mode in [false, true] {
                lua_settop(L, 2);
                lua_pushboolean(L, i32::from(mode));
                assert_eq!(resolve_address_only(L).unwrap(), mode);
                assert_eq!(lua_gettop(L), 3);
            }
            lua_settop(L, 2);
            lua_pushnil(L);
            assert!(resolve_address_only(L).is_err());
            lua_settop(L, 2);
            lua_pushinteger(L, 1);
            assert!(resolve_address_only(L).is_err());
            lua_settop(L, 2);
            lua_pushboolean(L, 1);
            lua_pushnil(L);
            assert!(resolve_address_only(L).is_err());
            assert_eq!(lua_gettop(L), 4);
            mlua::ffi::lua_close(L.cast());
        }
    }
    #[test]
    fn source_scope_address_only_keeps_full_owner_proof_and_same_pure_tail_without_factory() {
        for address_only in [false, true] {
            let (mut s, e) = fixture();
            let path = e.rows.borrow()[&7].path.clone();
            let h = s.keep(&e, 7, 5, path).unwrap();
            let before = *e.owner_calls.borrow();
            let factories = std::cell::Cell::new(0);
            let tails = std::cell::Cell::new(0);
            assert_eq!(
                s.resolve_return(
                    &e,
                    h,
                    address_only,
                    |address| {
                        assert_eq!(address, 7);
                        factories.set(factories.get() + 1);
                        Ok(())
                    },
                    || {
                        e.admit()?;
                        s.pure_hard_links(&e, h)?;
                        e.admit()?;
                        tails.set(tails.get() + 1);
                        Ok(())
                    }
                )
                .unwrap(),
                7
            );
            assert_eq!(factories.get(), u32::from(!address_only));
            assert_eq!(tails.get(), 1);
            assert_eq!(
                *e.owner_calls.borrow() - before,
                if address_only { 4 } else { 8 }
            );
        }
    }
    #[test]
    fn source_scope_address_only_refuses_original_identity_and_link_replacement() {
        for mutation in 0..10 {
            let (mut s, e) = fixture();
            let path = e.rows.borrow()[&7].path.clone();
            let h = s.keep(&e, 7, 5, path).unwrap();
            match mutation {
                0 => e.change(7, |r| r.id.weak += 1),
                1 => e.change(7, |r| r.id.name += 1),
                2 => e.change(7, |r| r.id.class_address += 1),
                3 => e.change(7, |r| r.flags |= GARBAGE),
                4 => e.change(7, |r| r.path.push_str("_new")),
                5 => e.change(7, |r| r.world = 99),
                6 => e.change(7, |r| r.owner = 2),
                7 => e.change(7, |r| r.parent = 6),
                8 => e.change(5, |r| r.root = 4),
                9 => e.change(4, |r| r.id.name += 1),
                _ => unreachable!(),
            }
            assert!(
                s.resolve_return(
                    &e,
                    h,
                    true,
                    |_| panic!("scalar factory"),
                    || panic!("invalid original tail")
                )
                .is_err()
            );
        }
    }
    #[test]
    fn source_scope_address_only_refuses_generation_changes_during_native_getters() {
        for owner_callback in [false, true] {
            let (mut s, e) = fixture();
            let path = e.rows.borrow()[&7].path.clone();
            let h = s.keep(&e, 7, 5, path).unwrap();
            if owner_callback {
                e.generation_on_owner.set(*e.owner_calls.borrow() + 1);
            } else {
                e.generation_on_world.set(e.world_calls.get() + 1);
            }
            assert!(
                s.resolve_return(
                    &e,
                    h,
                    true,
                    |_| panic!("scalar factory"),
                    || panic!("changed generation tail")
                )
                .is_err()
            );
        }
    }
    #[test]
    fn source_scope_address_only_final_pure_tail_cannot_accept_later_link_or_generation_changes() {
        for generation in [false, true] {
            let (mut s, e) = fixture();
            let path = e.rows.borrow()[&7].path.clone();
            let h = s.keep(&e, 7, 5, path).unwrap();
            assert!(
                s.resolve_return(
                    &e,
                    h,
                    true,
                    |_| panic!("scalar factory"),
                    || {
                        if generation {
                            e.generation_changed.set(true);
                        } else {
                            e.change(7, |r| r.parent = 6);
                        }
                        e.admit()?;
                        s.pure_hard_links(&e, h)?;
                        e.admit()
                    }
                )
                .is_err()
            );
        }
    }
    #[test]
    fn source_scope_fresh_factory_preserves_original_address_and_rechecks_after_callbacks() {
        let (mut s, e) = fixture();
        let path = e.rows.borrow()[&7].path.clone();
        let h = s.keep(&e, 7, 5, path).unwrap();
        let before_owner = *e.owner_calls.borrow();
        let factory_calls = std::cell::Cell::new(0);
        let finished = std::cell::Cell::new(false);
        assert_eq!(
            s.resolve_object(
                &e,
                h,
                |address| {
                    assert_eq!(address, 7);
                    factory_calls.set(factory_calls.get() + 1);
                    Ok(())
                },
                || {
                    assert!(
                        *e.owner_calls.borrow() > before_owner,
                        "full native owner checks retained"
                    );
                    s.pure_hard_links(&e, h)?;
                    finished.set(true);
                    Ok(())
                }
            )
            .unwrap(),
            7
        );
        assert_eq!(factory_calls.get(), 1);
        assert!(finished.get());
    }
    #[test]
    fn source_scope_fresh_factory_refuses_constructor_identity_path_world_and_link_mutations() {
        for mutation in 0..10 {
            let (mut s, e) = fixture();
            let path = e.rows.borrow()[&7].path.clone();
            let h = s.keep(&e, 7, 5, path).unwrap();
            let result = s.resolve_object(
                &e,
                h,
                |_| {
                    match mutation {
                        0 => e.change(7, |r| r.id.weak += 1),
                        1 => e.change(7, |r| r.id.name += 1),
                        2 => e.change(7, |r| r.id.class_address += 1),
                        3 => e.change(7, |r| r.flags |= GARBAGE),
                        4 => e.change(7, |r| r.path.push_str("_new")),
                        5 => e.change(7, |r| r.world = 99),
                        6 => e.change(7, |r| r.owner = 2),
                        7 => e.change(7, |r| r.parent = 6),
                        8 => e.change(5, |r| r.root = 4),
                        9 => e.change(4, |r| r.parent = 6),
                        _ => unreachable!(),
                    }
                    Ok(())
                },
                || s.pure_hard_links(&e, h),
            );
            assert!(
                result.is_err(),
                "constructor callback mutation {mutation} cannot bless the wrapper"
            );
        }
    }
    #[test]
    fn source_scope_fresh_factory_never_runs_for_invalid_original_or_after_factory_failure() {
        let (mut s, e) = fixture();
        let path = e.rows.borrow()[&7].path.clone();
        let h = s.keep(&e, 7, 5, path).unwrap();
        assert!(
            s.resolve_object(
                &e,
                h,
                |_| Err("factory refused".into()),
                || panic!("failed factory cannot finish")
            )
            .is_err()
        );
        e.change(7, |r| r.flags |= GARBAGE);
        assert!(
            s.resolve_object(
                &e,
                h,
                |_| panic!("retired original cannot construct"),
                || panic!("retired original cannot finish")
            )
            .is_err()
        );
    }
    #[test]
    fn source_scope_fresh_factory_final_generation_admission_is_required() {
        let (mut s, e) = fixture();
        let path = e.rows.borrow()[&7].path.clone();
        let h = s.keep(&e, 7, 5, path).unwrap();
        let generation = std::cell::Cell::new(s.dir_seq);
        assert!(
            s.resolve_object(
                &e,
                h,
                |_| {
                    generation.set(s.dir_seq + 1);
                    Ok(())
                },
                || {
                    s.pure_hard_links(&e, h)?;
                    if generation.get() == s.dir_seq {
                        Ok(())
                    } else {
                        Err("fixture original directory changed".into())
                    }
                }
            )
            .is_err()
        );
    }
    #[test]
    fn source_scope_callback_generation_changes_refuse_before_next_callback_or_factory() {
        for owner_callback in [false, true] {
            let (mut s, e) = fixture();
            let path = e.rows.borrow()[&7].path.clone();
            let h = s.keep(&e, 7, 5, path).unwrap();
            e.world_calls.set(0);
            let owner_before = *e.owner_calls.borrow();
            if owner_callback {
                e.generation_on_owner.set(owner_before + 1);
            } else {
                e.generation_on_world.set(1);
            }
            assert!(
                s.resolve_object(
                    &e,
                    h,
                    |_| panic!("changed callback generation cannot construct"),
                    || panic!("changed callback generation cannot return")
                )
                .is_err()
            );
            assert_eq!(
                *e.owner_calls.borrow() - owner_before,
                u32::from(owner_callback)
            );
            if !owner_callback {
                assert_eq!(e.world_calls.get(), 1);
            }
        }
        let (mut s, e) = fixture();
        let path = e.rows.borrow()[&7].path.clone();
        let h = s.keep(&e, 7, 5, path).unwrap();
        let worlds = std::cell::Cell::new(0);
        assert!(
            s.resolve_object(
                &e,
                h,
                |_| {
                    worlds.set(e.world_calls.get());
                    e.generation_changed.set(true);
                    Ok(())
                },
                || panic!("changed factory generation cannot return")
            )
            .is_err()
        );
        assert_eq!(
            e.world_calls.get(),
            worlds.get(),
            "fresh admission before any post-factory callback"
        );
    }
    #[test]
    fn source_scope_begin_owner_pure_tail_requires_final_admission() {
        let (s, e) = fixture();
        s.qualify_owners(&e).unwrap();
        let fields = e.field_calls.get();
        assert_eq!(fields, 3, "complete original owner/root/weapon links");
        e.generation_on_field.set(fields * 2);
        assert!(
            s.qualify_owners(&e).is_err(),
            "begin cannot publish after final pure-tail generation mutation"
        );
        assert_eq!(
            e.field_calls.get(),
            fields * 2,
            "mutation occurs in the final field, after all callbacks"
        );
        assert!(e.generation_changed.get());
    }
    #[test]
    fn source_scope_initial_path_lookup_brackets_hierarchy_and_rejects_callback_rename() {
        for callback in [1, 2] {
            let (mut s, e) = fixture();
            let path = e.rows.borrow()[&7].path.clone();
            e.rename_on_find.set(callback);
            assert!(s.keep(&e, 7, 5, path).is_err());
            assert!(s.components.is_empty());
            assert_eq!(
                *e.owner_calls.borrow(),
                0,
                "lookup mutation refuses before GetOwner"
            );
        }
    }
    #[test]
    fn source_scope_final_path_witness_rejects_later_native_getter_mutation() {
        for (profile, callback) in [(true, 3), (false, 5)] {
            let (mut s, e) = fixture();
            let path = e.rows.borrow()[&7].path.clone();
            let handle = s.keep(&e, 7, 5, path).unwrap();
            e.world_calls.set(0);
            e.rename_on_world.set(callback);
            let result = if profile {
                s.profile_guard(&e, handle).map(|()| 7)
            } else {
                s.resolve(&e, handle)
            };
            assert!(
                result.is_err(),
                "native getter mutation after early path check must fail"
            );
        }
    }
    #[test]
    fn source_scope_static_vertex_proof_never_defaults_unknown_to_native_asset() {
        use crate::native_presentation::VertexStateProof;
        for count in [0, 1, 16] {
            let proof = checked_vertex_state(VertexStateProof {
                material_count: 0,
                material_null_mask: 0,
                component_kind: 1,
                lod_info_count: count,
                no_override: 1,
                asset_present: 1,
            })
            .unwrap();
            assert!(proof.no_override);
            assert!(proof.asset_present);
            assert_eq!(proof.state(), "native_asset");
            assert_eq!(proof.lod_info_count, count);
        }
        assert!(
            !checked_vertex_state(VertexStateProof {
                material_count: 0,
                material_null_mask: 0,
                component_kind: 1,
                lod_info_count: 1,
                no_override: 0,
                asset_present: 1,
            })
            .unwrap()
            .no_override
        );
        for (count, state) in [(0, 0), (17, 1), (17, 0), (1, 2), (0, u32::MAX)] {
            assert!(
                checked_vertex_state(VertexStateProof {
                    material_count: 0,
                    material_null_mask: 0,
                    component_kind: 1,
                    lod_info_count: count,
                    no_override: state,
                    asset_present: 1,
                })
                .is_err()
            );
        }
    }
    #[test]
    fn source_scope_empty_static_presence_is_explicit_and_strict() {
        use crate::native_presentation::VertexStateProof;
        for count in [0, 1, 16] {
            let proof = checked_vertex_state(VertexStateProof {
                material_count: 0,
                material_null_mask: 0,
                component_kind: 1,
                lod_info_count: count,
                no_override: 1,
                asset_present: 0,
            })
            .unwrap();
            assert!(!proof.asset_present);
            assert!(proof.no_override);
            assert_eq!(proof.state(), "native_empty");
            assert_eq!(proof.lod_info_count, count);
        }
        let present = checked_vertex_state(VertexStateProof {
            material_count: 0,
            material_null_mask: 0,
            component_kind: 1,
            lod_info_count: 1,
            no_override: 0,
            asset_present: 1,
        })
        .unwrap();
        assert_eq!(present.state(), "captured_required");
        for (count, no_override, asset_present) in [
            (0, 0, 0),
            (1, 0, 0),
            (16, 0, 0),
            (17, 1, 0),
            (0, 1, 2),
            (0, 1, u32::MAX),
            (0, 2, 0),
        ] {
            assert!(
                checked_vertex_state(VertexStateProof {
                    material_count: 0,
                    material_null_mask: 0,
                    component_kind: 1,
                    lod_info_count: count,
                    no_override,
                    asset_present,
                })
                .is_err(),
                "invalid native empty proof {count}/{no_override}/{asset_present}"
            );
        }
    }
    #[test]
    fn source_scope_skeletal_empty_proof_preserves_material_null_mask() {
        use crate::native_presentation::VertexStateProof;
        for (count, mask) in [(0, 0), (3, 5), (32, u32::MAX)] {
            let proof = checked_vertex_state(VertexStateProof {
                lod_info_count: 0,
                no_override: 1,
                asset_present: 0,
                material_count: count,
                material_null_mask: mask,
                component_kind: 0,
            })
            .unwrap();
            assert_eq!(proof.state(), "native_empty_skeletal");
            assert_eq!(proof.material_count, count);
            assert_eq!(proof.material_null_mask, mask);
            assert_eq!(proof.component_kind, 0);
        }
        for (count, mask, kind, present, no_override) in [
            (0, 1, 0, 0, 1),
            (3, 8, 0, 0, 1),
            (31, u32::MAX, 0, 0, 1),
            (33, 0, 0, 0, 1),
            (0, 0, 2, 0, 1),
            (0, 0, 0, 1, 1),
            (0, 0, 0, 0, 0),
        ] {
            assert!(
                checked_vertex_state(VertexStateProof {
                    lod_info_count: 1,
                    no_override,
                    asset_present: present,
                    material_count: count,
                    material_null_mask: mask,
                    component_kind: kind,
                })
                .is_err()
            );
        }
    }
    #[test]
    fn source_scope_material_or_kind_change_between_native_censuses_refuses() {
        for mutation in 0..3 {
            let (mut scope, engine) = fixture();
            let path = engine.rows.borrow()[&7].path.clone();
            let handle = scope.keep(&engine, 7, 5, path).unwrap();
            let calls = std::cell::Cell::new(0);
            let result = capture_static_vertex_state(
                &scope,
                &engine,
                handle,
                || {
                    calls.set(calls.get() + 1);
                    let mut p = crate::native_presentation::VertexStateProof {
                        lod_info_count: 0,
                        no_override: 1,
                        asset_present: 0,
                        material_count: 3,
                        material_null_mask: 5,
                        component_kind: 0,
                    };
                    if calls.get() == 2 {
                        match mutation {
                            0 => p.material_count = 4,
                            1 => p.material_null_mask = 1,
                            _ => p.component_kind = 1,
                        }
                    }
                    Ok(p)
                },
                || panic!("changed material/kind cannot pass final acceptance"),
            );
            assert_eq!(calls.get(), 2);
            assert!(
                result.is_err(),
                "native raw material/kind mutation {mutation}"
            );
        }
    }
    #[test]
    fn source_scope_asset_presence_change_between_censuses_refuses() {
        for first_present in [false, true] {
            let (mut scope, engine) = fixture();
            let path = engine.rows.borrow()[&7].path.clone();
            let handle = scope.keep(&engine, 7, 5, path).unwrap();
            let calls = std::cell::Cell::new(0);
            let result = capture_static_vertex_state(
                &scope,
                &engine,
                handle,
                || {
                    calls.set(calls.get() + 1);
                    Ok(crate::native_presentation::VertexStateProof {
                        material_count: 0,
                        material_null_mask: 0,
                        component_kind: 1,
                        lod_info_count: 0,
                        no_override: 1,
                        asset_present: u32::from(if calls.get() == 1 {
                            first_present
                        } else {
                            !first_present
                        }),
                    })
                },
                || panic!("changed native presence must fail before final acceptance"),
            );
            assert_eq!(calls.get(), 2);
            assert_eq!(
                result.unwrap_err(),
                "source static vertex state changed during native getters"
            );
        }
    }
    #[test]
    fn source_scope_static_vertex_reproof_follows_callback_capable_owner_resolution() {
        for originally_present in [false, true] {
            let (mut scope, engine) = fixture();
            let path = engine.rows.borrow()[&7].path.clone();
            let handle = scope.keep(&engine, 7, 5, path).unwrap();
            engine.vertex_override.set(originally_present);
            let calls = std::cell::Cell::new(0);
            let result = capture_static_vertex_state(
                &scope,
                &engine,
                handle,
                || {
                    calls.set(calls.get() + 1);
                    if calls.get() == 1 {
                        engine
                            .override_on_owner
                            .set(*engine.owner_calls.borrow() + 1);
                    }
                    Ok(crate::native_presentation::VertexStateProof {
                        material_count: 0,
                        material_null_mask: 0,
                        component_kind: 1,
                        lod_info_count: 1,
                        no_override: u32::from(!engine.vertex_override.get()),
                        asset_present: 1,
                    })
                },
                || scope.pure_hard_links(&engine, handle),
            );
            assert_eq!(
                calls.get(),
                2,
                "late native owner callback needs a fresh census"
            );
            assert_ne!(engine.vertex_override.get(), originally_present);
            assert_eq!(
                result.unwrap_err(),
                "source static vertex state changed during native getters"
            );
        }
    }
    #[test]
    fn source_scope_static_vertex_lifetime_change_prevents_second_provider_call() {
        for mutation in 0..5 {
            let (mut scope, engine) = fixture();
            let path = engine.rows.borrow()[&7].path.clone();
            let handle = scope.keep(&engine, 7, 5, path).unwrap();
            let calls = std::cell::Cell::new(0);
            let result = capture_static_vertex_state(
                &scope,
                &engine,
                handle,
                || {
                    calls.set(calls.get() + 1);
                    engine.change(7, |row| match mutation {
                        0 => row.flags = GARBAGE,
                        1 => row.id.weak += 1,
                        2 => row.path.push_str("_replaced"),
                        3 => row.parent = 6,
                        _ => row.world = 8,
                    });
                    Ok(crate::native_presentation::VertexStateProof {
                        material_count: 0,
                        material_null_mask: 0,
                        component_kind: 1,
                        lod_info_count: 1,
                        no_override: 1,
                        asset_present: 1,
                    })
                },
                || scope.pure_hard_links(&engine, handle),
            );
            assert!(result.is_err(), "original lifetime mutation {mutation}");
            assert_eq!(calls.get(), 1, "no provider call on a replaced original");
        }
    }
    #[test]
    fn source_scope_static_vertex_original_sequence_finishes_with_native_reproof() {
        let (mut scope, engine) = fixture();
        let path = engine.rows.borrow()[&7].path.clone();
        let handle = scope.keep(&engine, 7, 5, path).unwrap();
        let calls = std::cell::Cell::new(0);
        let last_owner_calls = std::cell::Cell::new(0);
        let proof = capture_static_vertex_state(
            &scope,
            &engine,
            handle,
            || {
                calls.set(calls.get() + 1);
                last_owner_calls.set(*engine.owner_calls.borrow());
                Ok(crate::native_presentation::VertexStateProof {
                    material_count: 0,
                    material_null_mask: 0,
                    component_kind: 1,
                    lod_info_count: 0,
                    no_override: 1,
                    asset_present: 1,
                })
            },
            || scope.pure_hard_links(&engine, handle),
        )
        .unwrap();
        assert_eq!(calls.get(), 2);
        assert_eq!(last_owner_calls.get(), *engine.owner_calls.borrow());
        assert_eq!(
            proof,
            StaticVertexState {
                lod_info_count: 0,
                no_override: true,
                asset_present: true,
                material_count: 0,
                material_null_mask: 0,
                component_kind: 1,
            }
        );
        let refused = capture_static_vertex_state(
            &scope,
            &engine,
            0,
            || panic!("invalid original handle must not reach native provider"),
            || panic!("invalid original handle must not reach final validation"),
        );
        assert!(refused.is_err());
    }
    #[test]
    fn source_scope_static_vertex_late_provider_hard_link_change_refuses_without_callbacks() {
        for mutation in 0..4 {
            let (mut scope, engine) = fixture();
            let path = engine.rows.borrow()[&7].path.clone();
            let handle = scope.keep(&engine, 7, 5, path).unwrap();
            let original_component = engine.capture(7).unwrap();
            let calls = std::cell::Cell::new(0);
            let final_owner_calls = std::cell::Cell::new(0);
            let final_world_calls = std::cell::Cell::new(0);
            let result = capture_static_vertex_state(
                &scope,
                &engine,
                handle,
                || {
                    calls.set(calls.get() + 1);
                    if calls.get() == 2 {
                        // Simulate a final borrowed provider guard callback changing
                        // a hard link while retaining every original object identity.
                        match mutation {
                            0 => engine.change(5, |r| r.root = 4),
                            1 => engine.change(7, |r| r.parent = 6),
                            2 => engine.change(4, |r| r.parent = 6),
                            _ => engine.change(2, |r| r.root = 6),
                        }
                        final_owner_calls.set(*engine.owner_calls.borrow());
                        final_world_calls.set(engine.world_calls.get());
                    }
                    Ok(crate::native_presentation::VertexStateProof {
                        material_count: 0,
                        material_null_mask: 0,
                        component_kind: 1,
                        lod_info_count: 1,
                        no_override: 1,
                        asset_present: 1,
                    })
                },
                || scope.pure_hard_links(&engine, handle),
            );
            assert_eq!(calls.get(), 2, "mutation occurs in final native guard");
            assert!(
                result.is_err(),
                "late original hard-link mutation {mutation}"
            );
            assert_eq!(engine.capture(7).unwrap(), original_component);
            assert_eq!(*engine.owner_calls.borrow(), final_owner_calls.get());
            assert_eq!(engine.world_calls.get(), final_world_calls.get());
        }
    }
    #[test]
    fn source_scope_serial_zero_reuse_name_class_and_address_refuse() {
        for mutation in 0..5 {
            let (mut s, e) = fixture();
            let path = e.rows.borrow()[&7].path.clone();
            let h = s.keep(&e, 7, 5, path).unwrap();
            e.change(7, |r| match mutation {
                0 => r.id.name += 1,
                1 => r.id.class_weak += 1,
                2 => r.id.class_address += 1,
                3 => r.id.address += 1,
                _ => r.id.weak += 1,
            });
            assert!(s.resolve(&e, h).is_err(), "identity mutation {mutation}");
        }
    }
    #[test]
    fn source_scope_positive_serial_reuse_refuses() {
        let (mut s, e) = fixture();
        e.change(7, |r| r.id.weak |= 91 << 32);
        let path = e.rows.borrow()[&7].path.clone();
        let h = s.keep(&e, 7, 5, path).unwrap();
        e.change(7, |r| r.id.weak ^= 3 << 32);
        assert!(s.resolve(&e, h).is_err());
    }
    #[test]
    fn source_scope_owner_root_world_path_and_hard_link_mutations_refuse() {
        for mutation in 0..9 {
            let (mut s, e) = fixture();
            let path = e.rows.borrow()[&7].path.clone();
            let h = s.keep(&e, 7, 5, path).unwrap();
            match mutation {
                0 => e.change(7, |r| r.owner = 2),
                1 => e.change(5, |r| r.root = 4),
                2 => e.change(7, |r| r.world = 99),
                3 => e.change(7, |r| r.path.push_str("_REUSED")),
                4 => e.change(7, |r| r.parent = 6),
                5 => e.change(4, |r| r.id.name += 1),
                6 => e.change(6, |r| r.id.class_address += 1),
                7 => e.change(4, |r| r.owner = 5),
                _ => e.change(4, |r| r.world = 99),
            };
            assert!(s.resolve(&e, h).is_err(), "binding mutation {mutation}");
        }
    }
    #[test]
    fn source_scope_garbage_refuses_before_owner_process_event() {
        let (mut s, e) = fixture();
        let path = e.rows.borrow()[&7].path.clone();
        e.change(7, |r| r.flags = GARBAGE);
        let before = *e.owner_calls.borrow();
        assert!(s.keep(&e, 7, 0, path).is_err());
        assert_eq!(*e.owner_calls.borrow(), before);
    }
    #[test]
    fn source_scope_unknown_owner_and_handle_refuse() {
        let (mut s, e) = fixture();
        let path = e.rows.borrow()[&7].path.clone();
        e.change(7, |r| r.owner = 99);
        assert!(s.keep(&e, 7, 0, path).is_err());
        assert!(s.resolve(&e, 0).is_err());
        assert!(s.resolve(&e, 65).is_err());
    }
    #[test]
    fn source_scope_spline_guard_checks_original_links_without_owner_reentry() {
        let (mut s, e) = fixture();
        let path = e.rows.borrow()[&7].path.clone();
        let h = s.keep(&e, 7, 5, path).unwrap();
        let calls = *e.owner_calls.borrow();
        assert!(s.profile_guard(&e, h).is_ok());
        assert_eq!(*e.owner_calls.borrow(), calls);
        assert!(s.profile_guard(&e, 0).is_err());
        assert!(s.profile_guard(&e, 65).is_err());
        for mutation in 0..7 {
            let (mut s, e) = fixture();
            let path = e.rows.borrow()[&7].path.clone();
            let h = s.keep(&e, 7, 5, path).unwrap();
            let calls = *e.owner_calls.borrow();
            match mutation {
                0 => e.change(7, |r| r.flags = GARBAGE),
                1 => e.change(7, |r| r.id.name += 1),
                2 => e.change(7, |r| r.parent = 6),
                3 => e.change(5, |r| r.root = 4),
                4 => e.change(4, |r| r.world = 99),
                5 => e.change(7, |r| r.path.push_str("_reuse")),
                _ => e.change(6, |r| r.flags = GARBAGE),
            };
            assert!(s.profile_guard(&e, h).is_err(), "mutation {mutation}");
            assert_eq!(
                *e.owner_calls.borrow(),
                calls,
                "no recursive owner PE {mutation}"
            );
        }
    }

    // Synthetic schema/callback fixture: exercises the production cache paths,
    // not native rendering or engine physics parity.
    struct SchemaFixture {
        engine: Mock,
        cache: Rc<RefCell<SchemaCache>>,
        inspections: RefCell<(u32, u32, u32, u32)>, // find, params, property, pointer reads
        callback: std::cell::Cell<u8>,
        bad_property: std::cell::Cell<bool>,
        bad_parameters: std::cell::Cell<bool>,
        words: RefCell<HashMap<u64, [u64; 2]>>,
    }
    impl SchemaFixture {
        fn new() -> (Scope, Self) {
            let (mut scope, engine) = fixture();
            for (address, path) in [
                (8, "/Script/Engine.ActorComponent:GetOwner"),
                (9, "/Script/Engine.ActorComponent"),
                (30, "fixture original class"),
                (31, "fixture other original class"),
            ] {
                engine.rows.borrow_mut().insert(
                    address,
                    Row {
                        id: Identity {
                            weak: address,
                            address,
                            name: 100 + address,
                            class_weak: 30,
                            class_address: 30,
                        },
                        world: 1,
                        owner: 2,
                        root: 4,
                        parent: 0,
                        path: path.into(),
                        flags: 0,
                    },
                );
            }
            let cache = Rc::new(RefCell::new(SchemaCache::default()));
            scope.schema = cache.clone();
            (
                scope,
                Self {
                    engine,
                    cache,
                    inspections: RefCell::new((0, 0, 0, 0)),
                    callback: std::cell::Cell::new(0),
                    bad_property: std::cell::Cell::new(false),
                    bad_parameters: std::cell::Cell::new(false),
                    words: RefCell::new(HashMap::from([
                        (7, [4, 777]),
                        (6, [0, 666]),
                        (4, [0, 444]),
                    ])),
                },
            )
        }
    }
    impl Engine for SchemaFixture {
        fn capture(&self, address: u64) -> Result<Identity, String> {
            self.engine.capture(address)
        }
        fn verify(&self, id: Identity) -> Result<(), String> {
            self.engine.verify(id)
        }
        fn world(&self, id: Identity) -> Result<u64, String> {
            self.engine.world(id)
        }
        fn field(&self, id: Identity, name: &str) -> Result<u64, String> {
            cached_field(&self.cache, self, id, name)
        }
        fn owner(&self, id: Identity) -> Result<u64, String> {
            cached_owner(&self.cache, self, id)
        }
        fn find(&self, path: &str) -> Result<u64, String> {
            self.inspections.borrow_mut().0 += 1;
            self.engine.find(path)
        }
    }
    impl SchemaEngine for SchemaFixture {
        fn schema_name(&self, name: &str) -> Result<u64, String> {
            Ok(name
                .bytes()
                .fold(1u64, |n, b| n.wrapping_mul(31).wrapping_add(b as u64)))
        }
        fn property(&self, id: Identity, name: &str) -> Result<reflect::HsmpProp, String> {
            self.verify(id)?;
            self.inspections.borrow_mut().2 += 1;
            Ok(reflect::HsmpProp {
                name: self.schema_name(name)?,
                cls: self.schema_name("ObjectProperty")?,
                size: 8,
                offset: if self.bad_property.get() {
                    65529
                } else if id.class_address == 31 {
                    8
                } else {
                    0
                },
                ..Default::default()
            })
        }
        fn parameters(&self, function: Identity) -> Result<(Vec<reflect::HsmpProp>, i32), String> {
            self.verify(function)?;
            self.inspections.borrow_mut().1 += 1;
            Ok((
                vec![reflect::HsmpProp {
                    name: self.schema_name("ReturnValue")?,
                    cls: self.schema_name("ObjectProperty")?,
                    size: 8,
                    ..Default::default()
                }],
                if self.bad_parameters.get() { 16 } else { 8 },
            ))
        }
        fn read_pointer(&self, id: Identity, property: reflect::HsmpProp) -> Result<u64, String> {
            self.verify(id)?;
            self.inspections.borrow_mut().3 += 1;
            if property.name == self.schema_name("AttachParent")? {
                return Ok(self.words.borrow()[&id.address][property.offset as usize / 8]);
            }
            for name in ["Controller", "Pawn", "RootComponent", "Weapon R"] {
                if property.name == self.schema_name(name)? {
                    return self.engine.field(id, name);
                }
            }
            Ok(0)
        }
        fn dispatch_owner(
            &self,
            id: Identity,
            function: Identity,
            class: Identity,
        ) -> Result<u64, String> {
            self.verify(function)?;
            self.verify(class)?;
            let result = self.engine.owner(id)?;
            match self.callback.get() {
                1 => self.engine.change(8, |r| r.id.name += 1),
                2 => self.engine.change(9, |r| r.flags = GARBAGE),
                3 => self.engine.change(id.address, |r| r.flags = GARBAGE),
                4 => self.engine.change(8, |r| r.flags = GARBAGE),
                5 => self.engine.change(9, |r| r.id.name += 1),
                6 => self.engine.change(2, |r| r.world = 99),
                7 => self.engine.change(5, |r| r.root = 4),
                8 => {
                    self.words.borrow_mut().insert(7, [6, 777]);
                }
                _ => {}
            }
            Ok(result)
        }
    }
    #[test]
    fn source_scope_schema_reuses_inspection_but_reads_links_and_dispatches_each_time() {
        let (_, e) = SchemaFixture::new();
        let id = e.capture(7).unwrap();
        assert_eq!(e.field(id, "AttachParent").unwrap(), 4);
        e.words.borrow_mut().insert(7, [55, 777]);
        assert_eq!(e.field(id, "AttachParent").unwrap(), 55);
        assert_eq!(e.owner(id).unwrap(), 5);
        assert_eq!(e.owner(id).unwrap(), 5);
        assert_eq!(*e.inspections.borrow(), (2, 1, 1, 2));
        assert_eq!(*e.engine.owner_calls.borrow(), 2);
        let fresh = RefCell::new(SchemaCache::default());
        assert_eq!(cached_owner(&fresh, &e, id).unwrap(), 5);
        assert_eq!(*e.inspections.borrow(), (4, 2, 1, 2));
        assert_eq!(*e.engine.owner_calls.borrow(), 3);
    }
    #[test]
    fn source_scope_cached_properties_are_bound_to_original_exact_class() {
        let (_, e) = SchemaFixture::new();
        let first = e.capture(7).unwrap();
        assert_eq!(e.field(first, "AttachParent").unwrap(), 4);
        e.engine.change(6, |r| {
            r.id.class_weak = 31;
            r.id.class_address = 31;
        });
        let second = e.capture(6).unwrap();
        assert_eq!(e.field(second, "AttachParent").unwrap(), 666);
        assert_eq!(e.inspections.borrow().2, 2);
        let reads = e.inspections.borrow().3;
        e.engine.change(31, |r| r.id.name += 1);
        assert!(e.field(second, "AttachParent").is_err());
        assert_eq!(e.inspections.borrow().3, reads);
        e.engine.change(30, |r| r.flags = GARBAGE);
        assert!(e.field(first, "AttachParent").is_err());
        assert_eq!(e.inspections.borrow().3, reads);
    }
    #[test]
    fn source_scope_cached_function_class_and_component_mutation_prevent_dispatch() {
        for mutation in 0..5 {
            let (_, e) = SchemaFixture::new();
            let id = e.capture(7).unwrap();
            e.owner(id).unwrap();
            let calls = *e.engine.owner_calls.borrow();
            match mutation {
                0 => e.engine.change(8, |r| r.id.name += 1),
                1 => e.engine.change(9, |r| r.id.weak |= 91 << 32),
                2 => e.engine.change(8, |r| r.flags = GARBAGE),
                3 => e.engine.change(9, |r| r.flags = GARBAGE),
                _ => e.engine.change(7, |r| r.id.name += 1),
            }
            assert!(e.owner(id).is_err(), "cached mutation {mutation}");
            assert_eq!(*e.engine.owner_calls.borrow(), calls);
        }
    }
    #[test]
    fn source_scope_callback_function_class_component_and_links_are_requalified() {
        for mutation in 1..=8 {
            let (mut scope, e) = SchemaFixture::new();
            let path = e.engine.rows.borrow()[&7].path.clone();
            let h = scope.keep(&e, 7, 5, path).unwrap();
            let calls = *e.engine.owner_calls.borrow();
            e.callback.set(mutation);
            assert!(
                scope.resolve(&e, h).is_err(),
                "callback mutation {mutation}"
            );
            assert!(
                *e.engine.owner_calls.borrow() > calls,
                "callback must actually run"
            );
        }
    }
    #[test]
    fn source_scope_schema_abi_and_budget_are_bounded_before_reads_or_dispatch() {
        let (_, e) = SchemaFixture::new();
        let id = e.capture(7).unwrap();
        e.bad_property.set(true);
        assert!(e.field(id, "AttachParent").is_err());
        assert_eq!(e.inspections.borrow().3, 0);
        assert!(e.cache.borrow().fields.is_empty());
        e.bad_parameters.set(true);
        assert!(e.owner(id).is_err());
        assert_eq!(*e.engine.owner_calls.borrow(), 0);
        assert!(e.cache.borrow().owner.is_none());
        e.bad_property.set(false);
        assert_eq!(MAX_SCHEMA_FIELDS, 81);
        for i in 0..MAX_SCHEMA_FIELDS {
            e.field(id, &format!("fixture{i}")).unwrap();
        }
        let reads = e.inspections.borrow().3;
        assert!(e.field(id, "AttachParent").is_err());
        assert_eq!(e.inspections.borrow().3, reads);
        assert_eq!(e.cache.borrow().fields.len(), 81);
    }

    #[repr(C)]
    struct NativeObject {
        weak: u64,
        name: u64,
        flags: u32,
        class: *mut c_void,
        link: u64,
    }
    thread_local! {static OBJECTS:RefCell<HashMap<u32,usize>>=RefCell::new(HashMap::new());static NAME_READS:std::cell::Cell<u32>=const{std::cell::Cell::new(0)};}
    unsafe extern "C" fn resolve_object(weak: u64) -> *mut c_void {
        OBJECTS.with(|m| {
            let p = m.borrow().get(&(weak as u32)).copied().unwrap_or(0) as *mut NativeObject;
            if p.is_null() || weak >> 32 != 0 && unsafe { (*p).weak >> 32 } != weak >> 32 {
                std::ptr::null_mut()
            } else {
                p.cast()
            }
        })
    }
    unsafe extern "C" fn weak_object(p: *mut c_void) -> u64 {
        unsafe { (*p.cast::<NativeObject>()).weak }
    }
    unsafe extern "C" fn class_object(p: *mut c_void) -> *mut c_void {
        unsafe { (*p.cast::<NativeObject>()).class }
    }
    unsafe extern "C" fn name_object(p: *const c_void) -> *const u64 {
        NAME_READS.with(|n| n.set(n.get() + 1));
        unsafe { &(*p.cast::<NativeObject>()).name }
    }
    unsafe extern "C" fn flags_object(p: *const c_void) -> *const u32 {
        unsafe { &(*p.cast::<NativeObject>()).flags }
    }
    unsafe extern "C" fn world_object(_: *const c_void) -> *mut c_void {
        panic!("identity must not call native object world")
    }
    unsafe extern "C" fn unavailable_name(_: *const u16, _: i32) -> u64 {
        0
    }
    unsafe extern "C" fn unavailable_find(_: *const u16) -> *mut c_void {
        std::ptr::null_mut()
    }
    unsafe extern "C" fn unavailable_is(_: *mut c_void, _: *mut c_void) -> i32 {
        0
    }
    unsafe extern "C" fn unavailable_props(
        _: *mut c_void,
        _: *mut reflect::HsmpProp,
        _: i32,
        _: *mut i32,
    ) -> i32 {
        0
    }
    unsafe extern "C" fn unavailable_prop(
        _: *mut c_void,
        _: *const u16,
        _: *mut reflect::HsmpProp,
    ) -> i32 {
        0
    }
    unsafe extern "C" fn unavailable_call(_: *mut c_void, _: *mut c_void, _: *mut c_void) {
        panic!("identity must not call ProcessEvent")
    }
    fn with_native_identity(test: impl FnOnce(&Runtime<'_>, Identity, &mut NativeObject)) {
        let mut class = Box::new(NativeObject {
            weak: 30,
            name: 300,
            flags: 0,
            class: std::ptr::null_mut(),
            link: 0,
        });
        class.class = (&mut *class as *mut NativeObject).cast();
        let mut object = Box::new(NativeObject {
            weak: 7,
            name: 700,
            flags: 0,
            class: (&mut *class as *mut NativeObject).cast(),
            link: 111,
        });
        OBJECTS.with(|m| {
            let mut m = m.borrow_mut();
            m.clear();
            m.insert(30, (&mut *class as *mut NativeObject) as usize);
            m.insert(7, (&mut *object as *mut NativeObject) as usize);
        });
        let vt = HsmpReflect {
            abi: 1,
            _r: 0,
            fname: unavailable_name,
            find: unavailable_find,
            is_a: unavailable_is,
            class_of: class_object,
            props: unavailable_props,
            obj_prop: unavailable_prop,
            call: unavailable_call,
            weak: weak_object,
            resolve: resolve_object,
        };
        let n = Native::new();
        let x = Exports {
            name: name_object,
            flags: flags_object,
            world: world_object,
        };
        let e = Runtime {
            n: &n,
            vt: &vt,
            x,
            key: b"fixture",
            base: vec![],
            reference: reference(-1, 1, 1).unwrap(),
            dir_seq: 1,
            index: 0,
            schema: Rc::new(RefCell::new(SchemaCache::default())),
        };
        let id = Identity {
            weak: 7,
            address: (&mut *object as *mut NativeObject) as u64,
            name: 700,
            class_weak: 30,
            class_address: (&mut *class as *mut NativeObject) as u64,
        };
        test(&e, id, &mut object);
        OBJECTS.with(|m| m.borrow_mut().clear());
    }
    #[test]
    fn source_scope_native_metadata_serial_zero_identity_and_reuse() {
        with_native_identity(|e, id, o| {
            assert!(e.identity(id).is_ok());
            o.name += 1;
            assert!(e.identity(id).is_err());
            o.name -= 1;
            o.class = std::ptr::null_mut();
            assert!(e.identity(id).is_err());
        });
    }
    #[test]
    fn source_scope_native_garbage_precedes_metadata_or_engine_calls() {
        with_native_identity(|e, id, o| {
            NAME_READS.with(|n| n.set(0));
            o.flags = GARBAGE;
            assert!(e.identity(id).is_err());
            assert_eq!(NAME_READS.with(|n| n.get()), 0);
        });
    }
    #[test]
    fn source_scope_native_positive_serial_is_exact() {
        with_native_identity(|e, mut id, o| {
            o.weak |= 91 << 32;
            id.weak = o.weak;
            assert!(e.identity(id).is_ok());
            o.weak = 7 | (92 << 32);
            assert!(e.identity(id).is_err());
        });
    }
    #[test]
    fn source_scope_runtime_scalar_batch_reads_fresh_without_nested_admissions() {
        with_native_identity(|e, id, o| {
            let class_pointer = unsafe { (e.vt.resolve)(id.class_weak) };
            let class = Identity {
                weak: id.class_weak,
                address: id.class_address,
                name: unsafe { *(e.x.name)(class_pointer) },
                class_weak: id.class_weak,
                class_address: id.class_address,
            };
            e.schema.borrow_mut().fields.push(FieldSchema {
                class,
                name: "RootComponent".into(),
                property: reflect::HsmpProp {
                    size: 8,
                    offset: std::mem::offset_of!(NativeObject, link) as i32,
                    ..Default::default()
                },
            });
            let _trace = ProfileOperation::begin(reference(1, 2, 3).unwrap(), 4, 5);
            NAME_READS.with(|n| n.set(0));
            for value in 1..=128 {
                o.link = value;
                e.verify(id).unwrap();
                assert_eq!(e.field(id, "RootComponent").unwrap(), value);
            }
            assert_eq!(
                PROFILE_TRACE.with(|p| p.borrow().as_ref().unwrap().trace.admissions),
                0,
                "original scalar identities/links do not clone directory repeatedly"
            );
            assert!(
                NAME_READS.with(|n| n.get()) > 128,
                "original FName/class identities still read freshly"
            );
            o.flags = GARBAGE;
            assert!(e.field(id, "RootComponent").is_err());
            o.flags = 0;
            unsafe {
                (*class_pointer.cast::<NativeObject>()).name += 1;
            }
            assert!(
                e.field(id, "RootComponent").is_err(),
                "original class layout cannot be reused"
            );
            // This Native is deliberately unadmitted. Every callback must
            // freshly refuse before the panic/no-op native fixture functions.
            assert!(e.world(id).is_err());
            assert!(
                e.schema_name("ReturnValue").is_err(),
                "name failure propagates, never zero/default"
            );
            assert!(e.find("/Script/Engine.ActorComponent").is_err());
            assert!(e.property(id, "Unknown").is_err());
            assert!(e.parameters(id).is_err());
            assert!(e.capture(id.address).is_err());
            assert!(e.dispatch_owner(id, id, class).is_err());
            assert_eq!(
                PROFILE_TRACE.with(|p| p.borrow().as_ref().unwrap().trace.admissions),
                7
            );
            assert_eq!(
                PROFILE_TRACE.with(|p| p.borrow().as_ref().unwrap().trace.events),
                0
            );
        });
    }
    #[test]
    fn source_scope_pure_profile_reads_cached_schema_fresh_and_forbids_dispatch() {
        with_native_identity(|e, id, o| {
            let class_pointer = unsafe { (e.vt.resolve)(id.class_weak) };
            let class = Identity {
                weak: id.class_weak,
                address: id.class_address,
                name: unsafe { *(e.x.name)(class_pointer) },
                class_weak: id.class_weak,
                class_address: id.class_address,
            };
            e.schema.borrow_mut().fields.push(FieldSchema {
                class,
                name: "RootComponent".into(),
                property: reflect::HsmpProp {
                    size: 8,
                    offset: std::mem::offset_of!(NativeObject, link) as i32,
                    ..Default::default()
                },
            });
            let pure = ProfileGuardEngine { runtime: e };
            let _trace = ProfileOperation::begin(
                EntityRef {
                    epoch: 1,
                    id: 2,
                    incarnation: 3,
                },
                4,
                5,
            );
            assert!(pure.verify(id).is_ok());
            assert_eq!(pure.field(id, "RootComponent").unwrap(), 111);
            o.link = 222;
            assert_eq!(pure.field(id, "RootComponent").unwrap(), 222);
            assert_eq!(
                PROFILE_TRACE.with(|p| p.borrow().as_ref().unwrap().trace.admissions),
                0,
                "pure reads do not repeat Runtime admission"
            );
            assert!(pure.capture(id.class_address).is_err());
            assert!(pure.field(id, "AttachParent").is_err());
            assert!(pure.owner(id).is_err());
            assert!(pure.find("fixture").is_err());
            assert!(pure.path_matches(id, "fixture", None).is_err());
            assert!(pure.parameters(id).is_err());
            assert!(pure.dispatch_owner(id, id, class).is_err());
            assert_eq!(
                PROFILE_TRACE.with(|p| p.borrow().as_ref().unwrap().trace.events),
                0,
                "pure engine cannot dispatch PE"
            );
            o.flags = GARBAGE;
            assert!(pure.field(id, "RootComponent").is_err());
            o.flags = 0;
            o.name += 1;
            assert!(pure.field(id, "RootComponent").is_err());
            o.name -= 1;
            unsafe {
                (*class_pointer.cast::<NativeObject>()).name += 1;
            }
            assert!(
                pure.field(id, "RootComponent").is_err(),
                "cached class FName reuse rejects original offset"
            );
        });
    }
    thread_local! {
        static PATH_EXPECTED:RefCell<Option<PathWitness>>=const{RefCell::new(None)};
        static PATH_CALLBACK_GARBAGE:std::cell::Cell<bool>=const{std::cell::Cell::new(false)};
        static PATH_CALLBACK_COUNT:std::cell::Cell<u32>=const{std::cell::Cell::new(0)};
    }
    // This fixture checks the Rust/C argument contract only. The C++ tests
    // exercise the real hard Outer walk and every original hierarchy node.
    unsafe extern "C" fn path_fixture_reader(
        nodes: *const PathNode,
        count: u32,
        capture: u32,
        output: *mut PathNode,
        capacity: u32,
        output_count: *mut u32,
        package_name: *mut u64,
        _: *mut std::ffi::c_char,
        _: u32,
    ) -> i32 {
        PATH_CALLBACK_COUNT.with(|c| c.set(c.get() + 1));
        if capture != 0
            || nodes.is_null()
            || package_name.is_null()
            || !output.is_null()
            || capacity != 0
            || !output_count.is_null()
        {
            return -1;
        }
        let valid = PATH_EXPECTED.with(|expected| {
            let expected = expected.borrow();
            let Some(expected) = expected.as_ref() else {
                return false;
            };
            (unsafe { std::slice::from_raw_parts(nodes, count as usize) }) == expected.nodes
                && unsafe { *package_name } == expected.package_name
        });
        if !valid {
            return -1;
        }
        if PATH_CALLBACK_GARBAGE.with(|c| c.get()) {
            unsafe {
                (*((*nodes).address as *mut NativeObject)).flags = GARBAGE;
            }
        }
        1
    }
    #[test]
    fn source_scope_native_path_witness_marshaling_and_post_callback_qualification() {
        struct Restore(*mut c_void);
        impl Drop for Restore {
            fn drop(&mut self) {
                PATH_READER.store(self.0, Ordering::Release);
            }
        }
        let _restore =
            Restore(PATH_READER.swap(path_fixture_reader as *mut c_void, Ordering::AcqRel));
        with_native_identity(|e, id, object| {
            let witness = PathWitness {
                nodes: vec![PathNode {
                    weak: id.weak,
                    address: id.address,
                    name: id.name,
                    class_weak: id.class_weak,
                    class_address: id.class_address,
                    class_name: 300,
                }],
                package_name: 0x100000077,
            };
            PATH_EXPECTED.with(|p| *p.borrow_mut() = Some(witness.clone()));
            PATH_CALLBACK_GARBAGE.with(|p| p.set(false));
            PATH_CALLBACK_COUNT.with(|p| p.set(0));
            let _trace = ProfileOperation::begin(e.reference, e.dir_seq, 1);
            let pure = ProfileGuardEngine { runtime: e };
            assert!(
                pure.path_matches(id, "initial exact path", Some(&witness))
                    .unwrap()
            );
            assert_eq!(
                PROFILE_TRACE.with(|p| p.borrow().as_ref().unwrap().trace.finds),
                0
            );
            assert_eq!(
                PROFILE_TRACE.with(|p| p.borrow().as_ref().unwrap().trace.admissions),
                0
            );
            let mut changed = witness.clone();
            changed.nodes[0].address += 1;
            let calls = PATH_CALLBACK_COUNT.with(|p| p.get());
            assert!(
                pure.path_matches(id, "ignored text", Some(&changed))
                    .is_err()
            );
            assert_eq!(
                PATH_CALLBACK_COUNT.with(|p| p.get()),
                calls,
                "root mismatch refuses before reader"
            );
            changed = witness.clone();
            changed.package_name ^= 1;
            assert!(
                pure.path_matches(id, "ignored text", Some(&changed))
                    .is_err()
            );
            assert!(pure.path_matches(id, "ignored text", None).is_err());
            PATH_CALLBACK_GARBAGE.with(|p| p.set(true));
            assert!(
                pure.path_matches(id, "ignored text", Some(&witness))
                    .is_err(),
                "post-reader original garbage refuses"
            );
            object.flags = 0;
            PATH_CALLBACK_GARBAGE.with(|p| p.set(false));
            PATH_READER.store(std::ptr::null_mut(), Ordering::Release);
            assert!(
                pure.path_matches(id, "ignored text", Some(&witness))
                    .is_err(),
                "missing native reader never falls back to global lookup"
            );
        });
    }
    #[test]
    fn source_scope_profile_batch_keeps_outer_role_admission_before_native_reads() {
        with_native_identity(|e, id, _| {
            let scope = Scope {
                id: 1,
                reference: e.reference,
                dir_seq: e.dir_seq,
                index: e.index,
                key: e.key.to_vec(),
                world: id,
                pawn: id,
                controller: id,
                owners: HashMap::new(),
                components: vec![],
                schema: e.schema.clone(),
            };
            let engine = runtime(e.n, e.vt, e.x, &scope);
            let context = SplineScopeGuard {
                scope: &scope,
                engine,
                handle: 1,
            };
            NAME_READS.with(|n| n.set(0));
            assert!(
                context.valid().is_err(),
                "fixture has no admitted source role/thread"
            );
            assert_eq!(
                NAME_READS.with(|n| n.get()),
                0,
                "outer admission fails before native identity reads"
            );
        });
    }
    #[test]
    fn source_scope_static_vertex_api_refuses_unadmitted_source_before_provider() {
        with_native_identity(|engine, id, _| {
            let scope = Scope {
                id: 1,
                reference: engine.reference,
                dir_seq: engine.dir_seq,
                index: engine.index,
                key: engine.key.to_vec(),
                world: id,
                pawn: id,
                controller: id,
                owners: HashMap::new(),
                components: vec![],
                schema: engine.schema.clone(),
            };
            NAME_READS.with(|reads| reads.set(0));
            assert!(
                capture_static_vertex_state(
                    &scope,
                    engine,
                    1,
                    || { panic!("unadmitted client cannot receive a native asset proof") },
                    || panic!("unadmitted client cannot finish a native asset proof")
                )
                .is_err()
            );
            assert_eq!(NAME_READS.with(|reads| reads.get()), 0);
        });
    }
}
