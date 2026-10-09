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
    fn capture(&self, address: u64) -> Result<Identity, String>;
    fn verify(&self, id: Identity) -> Result<(), String>;
    fn world(&self, id: Identity) -> Result<u64, String>;
    fn field(&self, id: Identity, name: &str) -> Result<u64, String>;
    fn owner(&self, id: Identity) -> Result<u64, String>;
    fn find(&self, path: &str) -> Result<u64, String>;
}
trait SchemaEngine: Engine {
    fn schema_name(&self, name: &str) -> u64;
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
            if property.name != e.schema_name(name)
                || property.cls != e.schema_name("ObjectProperty")
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
                || properties[0].name != e.schema_name("ReturnValue")
                || properties[0].cls != e.schema_name("ObjectProperty")
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
}
#[derive(Clone, Copy, PartialEq, Eq)]
struct Parent {
    object: Identity,
    owner: u64,
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
        if e.find(&c.path)? != c.object.address || e.world(c.object)? != self.world.address {
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
        self.base(e)
    }
    fn base(&self, e: &impl Engine) -> Result<(), String> {
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
        Ok(())
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
            Some(Parent { object, owner })
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
        if e.find(&c.path)? != c.object.address
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
        Ok(c.object.address)
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
impl Runtime<'_> {
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
        self.admit()?;
        self.identity(id)?;
        self.admit()
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
}
impl SchemaEngine for Runtime<'_> {
    fn schema_name(&self, name: &str) -> u64 {
        unsafe { (self.vt.fname)(reflect::wide(name).as_ptr(), 1) }
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
        self.admit()?;
        let p = self.identity(id)?;
        unsafe {
            let value =
                std::ptr::read_unaligned((p as *const u8).add(prop.offset as usize).cast::<u64>());
            self.identity(id)?;
            self.admit()?;
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
        self.scope.profile_guard(e, self.handle)?;
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
impl Native {
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
            for address in s.owners.keys() {
                s.qualify_owner(&e, *address)?;
            }
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
        unsafe { self.source_scope_op(L, false) }
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
            Ok(self.row(id)?.world)
        }
        fn field(&self, id: Identity, name: &str) -> Result<u64, String> {
            let r = self.row(id)?;
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
            let r = self.row(id)?;
            *self.owner_calls.borrow_mut() += 1;
            Ok(r.owner)
        }
        fn find(&self, path: &str) -> Result<u64, String> {
            Ok(self
                .rows
                .borrow()
                .values()
                .find(|r| r.path == path)
                .map_or(0, |r| r.id.address))
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
                    words: RefCell::new(HashMap::from([(7, [4, 777]), (6, [0, 666])])),
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
        fn schema_name(&self, name: &str) -> u64 {
            name.bytes()
                .fold(1u64, |n, b| n.wrapping_mul(31).wrapping_add(b as u64))
        }
        fn property(&self, id: Identity, name: &str) -> Result<reflect::HsmpProp, String> {
            self.verify(id)?;
            self.inspections.borrow_mut().2 += 1;
            Ok(reflect::HsmpProp {
                name: self.schema_name(name),
                cls: self.schema_name("ObjectProperty"),
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
                    name: self.schema_name("ReturnValue"),
                    cls: self.schema_name("ObjectProperty"),
                    size: 8,
                    ..Default::default()
                }],
                if self.bad_parameters.get() { 16 } else { 8 },
            ))
        }
        fn read_pointer(&self, id: Identity, property: reflect::HsmpProp) -> Result<u64, String> {
            self.verify(id)?;
            self.inspections.borrow_mut().3 += 1;
            if property.name == self.schema_name("AttachParent") {
                return Ok(self.words.borrow()[&id.address][property.offset as usize / 8]);
            }
            for name in ["Controller", "Pawn", "RootComponent", "Weapon R"] {
                if property.name == self.schema_name(name) {
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
        });
        class.class = (&mut *class as *mut NativeObject).cast();
        let mut object = Box::new(NativeObject {
            weak: 7,
            name: 700,
            flags: 0,
            class: (&mut *class as *mut NativeObject).cast(),
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
}
