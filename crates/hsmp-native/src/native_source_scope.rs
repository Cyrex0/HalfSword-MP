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
};

const MAX_COMPONENTS: usize = 64;
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
}
impl Scope {
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
}
impl Runtime<'_> {
    fn admit(&self) -> Result<(), String> {
        if !self.n.native_host.is_host()
            || self.n.poisoned
            || self.n.game_thread != Some(std::thread::current().id())
            || self.n.world_key.as_deref() != Some(self.key)
        {
            return Err("source scope role/thread/world admission".into());
        }
        let d = self
            .n
            .native_host
            .directory()
            .ok_or("source scope directory unavailable")?;
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
        let result = unsafe { (self.x.world)(p) } as u64;
        self.identity(id)?;
        self.admit()?;
        Ok(result)
    }
    fn field(&self, id: Identity, name: &str) -> Result<u64, String> {
        self.admit()?;
        let p = self.identity(id)?;
        unsafe {
            let mut prop = reflect::HsmpProp::default();
            if (self.vt.obj_prop)(p, reflect::wide(name).as_ptr(), &mut prop) != 1
                || prop.cls != (self.vt.fname)(reflect::wide("ObjectProperty").as_ptr(), 1)
                || prop.size != 8
                || prop.offset < 0
                || prop.offset > 65528
            {
                return Err(format!("source scope hard {name} property ABI"));
            }
            self.identity(id)?;
            self.admit()?;
            let value =
                std::ptr::read_unaligned((p as *const u8).add(prop.offset as usize).cast::<u64>());
            self.identity(id)?;
            self.admit()?;
            Ok(value)
        }
    }
    fn owner(&self, id: Identity) -> Result<u64, String> {
        self.admit()?;
        unsafe {
            let class = (self.vt.find)(reflect::wide("/Script/Engine.ActorComponent").as_ptr());
            let function =
                (self.vt.find)(reflect::wide("/Script/Engine.ActorComponent:GetOwner").as_ptr());
            let f = self.capture(function as u64)?;
            let cls = self.capture(class as u64)?;
            let (props, size) = crate::sample::props_of(self.vt, self.identity(f)?);
            if size != 8
                || props.len() != 1
                || props[0].name != (self.vt.fname)(reflect::wide("ReturnValue").as_ptr(), 1)
                || props[0].cls != (self.vt.fname)(reflect::wide("ObjectProperty").as_ptr(), 1)
                || props[0].offset != 0
                || props[0].size != 8
            {
                return Err("source scope GetOwner ABI".into());
            }
            self.admit()?;
            let p = self.identity(id)?;
            if (self.vt.is_a)(p, self.identity(cls)?) != 1 {
                return Err("source scope component class".into());
            }
            let mut result = 0u64;
            self.admit()?;
            self.identity(f)?;
            self.identity(id)?;
            (self.vt.call)(p, function, (&mut result as *mut u64).cast());
            self.identity(id)?;
            self.admit()?;
            Ok(result)
        }
    }
    fn find(&self, path: &str) -> Result<u64, String> {
        self.admit()?;
        let p = unsafe { (self.vt.find)(reflect::wide(path).as_ptr()) } as u64;
        self.admit()?;
        Ok(p)
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
impl Native {
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
