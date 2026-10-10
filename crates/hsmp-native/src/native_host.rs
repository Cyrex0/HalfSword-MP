//! In-process network binding. Only immutable DTOs reach the socket thread.
use crate::{lua::*, native::Native};
use hsmp_server::{native_service::{HostHandle, ClientHandle}, native_wire::{self as w, EntityRef, InputFrame, World}};
use std::ffi::c_int;
use std::path::Path;
use std::sync::Arc;

#[derive(PartialEq)]
enum RuntimeRole{Legacy,Host,Client,Invalid}
pub struct ServiceState {
    pub host: Option<HostHandle>, pub client: Option<ClientHandle>,
    parent: Option<Arc<hsmp_ipc::shm::ProcessHandle>>, parent_checked: bool, parent_failed: bool,
    role: RuntimeRole,
    pub(crate) gameplay: crate::native_gameplay::State,
}
impl Default for ServiceState{
    fn default()->Self{
        let role=match std::env::var("HSMP_RUNTIME_ROLE").as_deref(){Ok("native_worker")=>RuntimeRole::Host,Ok("native_client")=>RuntimeRole::Client,Err(std::env::VarError::NotPresent)|Ok("")|Ok("client")=>RuntimeRole::Legacy,_=>RuntimeRole::Invalid};
        Self{host:None,client:None,parent:None,parent_checked:false,parent_failed:false,role,gameplay:Default::default()}
    }
}
impl ServiceState {
    pub(crate) fn is_host(&self)->bool{self.role==RuntimeRole::Host}
    pub(crate) fn is_client(&self)->bool{self.role==RuntimeRole::Client}
    pub fn publish_world(&self, world: World) -> Result<(), &'static str> { self.host.as_ref().ok_or("not a native host")?.publish_world(world) }
    pub fn directory(&self) -> Option<w::Directory> { self.host.as_ref().and_then(HostHandle::directory).or_else(|| self.client.as_ref().and_then(ClientHandle::directory)) }
    fn parent_alive(&mut self)->bool {
        if !self.parent_checked {
            self.parent_checked=true;
            if let Some(value)=std::env::var_os("HSMP_NATIVE_PARENT_PID") {
                match value.to_str().and_then(|s|s.parse::<u32>().ok()).filter(|p|*p!=0).and_then(|pid|hsmp_ipc::shm::ProcessHandle::open(pid).ok()) {
                    Some(handle)=>self.parent=Some(Arc::new(handle)),None=>self.parent_failed=true,
                }
            }
        }
        !self.parent_failed&&self.parent.as_ref().is_none_or(|p|p.is_alive())
    }
}
unsafe fn field_int(L: *mut lua_State, t: c_int, key: &str) -> Option<i64> {
    unsafe { rawget_str(L,t,key); let v=arg_int(L,-1); pop(L,1); v }
}
unsafe fn push_ref(L: *mut lua_State, t: c_int, r: EntityRef) {
    unsafe { set_int(L,t,"epoch",r.epoch as i64); set_int(L,t,"id",r.id as i64); set_int(L,t,"incarnation",r.incarnation as i64); }
}
unsafe fn array_field(L:*mut lua_State,t:c_int,key:&str,values:impl IntoIterator<Item=f64>){
    unsafe{lua_createtable(L,0,0);let a=lua_gettop(L);fill_array(L,a,values.into_iter());rawset_str(L,t,key);}
}
unsafe fn push_decoded_pose(L:*mut lua_State,row:c_int,pose:&[u8],tick:u32){
    unsafe{
        let Some(full)=hsmp_pose::posecodec::v2::decode(pose) else{return;};
        let frame=hsmp_pose::poseplay::Frame::from_v2(tick,&full);
        set_int(L,row,"mask",frame.mask as i64);set_int(L,row,"vmask",frame.vmask as i64);
        set_num(L,row,"ts",frame.ts);set_num(L,row,"k",full.k as f64);set_num(L,row,"step_ms",full.step as f64);
        lua_createtable(L,25,0);let bones=lua_gettop(L);let mut flat=[0f64;25*13];
        for i in 0..25{lua_createtable(L,13,0);let b=lua_gettop(L);let mut vals=[0f64;13];for j in 0..7{vals[j]=frame.b[i][j] as f64;}for j in 0..6{vals[j+7]=frame.vel[i][j] as f64;}flat[i*13..(i+1)*13].copy_from_slice(&vals);fill_array(L,b,vals.into_iter());lua_rawseti(L,bones,i as i64+1);}rawset_str(L,row,"b");array_field(L,row,"flat_b",flat);
        if let Some(c)=full.control.as_ref(){lua_createtable(L,0,10);let ct=lua_gettop(L);set_int(L,ct,"flags",c.flags as i64);set_int(L,ct,"grip_r",c.grip_r as i64);set_int(L,ct,"grip_l",c.grip_l as i64);set_num(L,ct,"ctrl_pitch",c.ctrl_pitch as f64);set_num(L,ct,"ctrl_yaw",c.ctrl_yaw as f64);array_field(L,ct,"scalars",c.scalars.iter().map(|x|*x as f64));array_field(L,ct,"aim",c.aim.iter().map(|x|*x as f64));set_int(L,ct,"ik_world",c.ik_world.iter().enumerate().fold(0u32,|a,(i,w)|a|((*w as u32)<<i)) as i64);lua_createtable(L,4,0);let ik=lua_gettop(L);for (i,v) in c.ik.iter().enumerate(){lua_createtable(L,3,0);let a=lua_gettop(L);fill_array(L,a,v.iter().map(|x|*x as f64));lua_rawseti(L,ik,i as i64+1);}rawset_str(L,ct,"ik");rawset_str(L,row,"control");}
        lua_createtable(L,full.weapons.len() as c_int,0);let weapons=lua_gettop(L);
        for (i,w) in full.weapons.iter().enumerate(){lua_createtable(L,0,8);let wt=lua_gettop(L);set_int(L,wt,"hands",w.hands as i64);set_int(L,wt,"class_id",w.id as i64);array_field(L,wt,"p",w.p.iter().map(|x|*x as f64));array_field(L,wt,"q",w.q.iter().map(|x|*x as f64));array_field(L,wt,"v",w.v.iter().map(|x|*x as f64));array_field(L,wt,"w",w.w.iter().map(|x|*x as f64));array_field(L,wt,"base",w.base.iter().map(|x|*x as f64));array_field(L,wt,"tip",w.tip.iter().map(|x|*x as f64));lua_rawseti(L,weapons,i as i64+1);}rawset_str(L,row,"weapons");
    }
}
impl Native {
    pub unsafe fn native_client_status(&mut self,L:*mut lua_State)->c_int{unsafe{
        let Some(c)=self.native_host.client.as_ref() else{return nil_err(L,"not a native client");};
        lua_createtable(L,0,3);let t=lua_gettop(L);set_bool(L,t,"connected",c.connected());set_int(L,t,"peer_id",c.peer_id() as i64);set_str(L,t,"error",&c.error());1
    }}
    pub unsafe fn host_world_changed(&mut self,L:*mut lua_State)->c_int{
        unsafe{let Some(h)=self.native_host.host.as_ref() else{return nil_err(L,"not a native host");};h.world_changed();lua_pushboolean(L,1);1}
    }
    pub unsafe fn host_parent_alive(&mut self,L:*mut lua_State)->c_int {
        unsafe{lua_pushboolean(L,self.native_host.parent_alive() as c_int);1}
    }
    pub unsafe fn host_stop(&mut self, L: *mut lua_State) -> c_int {
        unsafe {
            self.native_host.host.take(); self.native_host.client.take();
            lua_pushboolean(L,1);1
        }
    }
    pub unsafe fn host_start(&mut self, L: *mut lua_State) -> c_int {
        unsafe {
            if self.native_host.role!=RuntimeRole::Host { return nil_err(L,"host role required"); }
            if !self.native_host.parent_alive(){return nil_err(L,"native supervisor unavailable");}
            if self.native_host.client.is_some() { return nil_err(L,"client already running"); }
            if self.native_host.host.is_some() { lua_pushboolean(L,1); return 1; }
            let (Some(bind),Some(dir),Some(arena))=(arg_str(L,1),arg_str(L,2),arg_str(L,3)) else { return nil_err(L,"host arguments"); };
            let Ok(bind)=bind.parse() else {return nil_err(L,"host bind");};
            let mode = match hsmp_server::native_mode::Mode::parse(arg_str(L,4).unwrap_or("pvp")) { Ok(mode) => mode, Err(reason) => return nil_err(L,reason) };
            match HostHandle::start_with_parent_mode(bind,Path::new(dir),arena,self.native_host.parent.clone(),mode) {
                Ok(h)=>{self.native_host.host=Some(h);lua_pushboolean(L,1);1},
                Err(e)=>nil_err(L,&format!("native host: {e}")),
            }
        }
    }
    pub unsafe fn client_start(&mut self, L: *mut lua_State) -> c_int {
        unsafe {
            if self.native_host.role != RuntimeRole::Client {
                return nil_err(L, "native client role required");
            }
            if self.native_host.host.is_some() {
                return nil_err(L, "host already running");
            }
            if self.native_host.client.is_some() {
                lua_pushboolean(L, 1);
                return 1;
            }
            let (Some(addr), Some(dir), Some(nick)) = (arg_str(L, 1), arg_str(L, 2), arg_str(L, 3))
            else {
                return nil_err(L, "client arguments");
            };
            let Ok(addr) = addr.parse() else {
                return nil_err(L, "client address");
            };
            let pinned = match arg_str(L, 4) {
                None | Some("") => None,
                Some(s) => {
                    if s.len() != 64 {
                        return nil_err(L, "server key");
                    }
                    let mut key = [0u8; 32];
                    for (i, x) in key.iter_mut().enumerate() {
                        let Some(part) = s.get(i * 2..i * 2 + 2) else {
                            return nil_err(L, "server key");
                        };
                        let Ok(v) = u8::from_str_radix(part, 16) else {
                            return nil_err(L, "server key");
                        };
                        *x = v;
                    }
                    Some(key)
                }
            };
            let started = if arg_str(L, 5) == Some("gameplay") {
                ClientHandle::start_gameplay(addr, Path::new(dir), nick, pinned)
            } else {
                ClientHandle::start_presentation(addr, Path::new(dir), nick, pinned)
            };
            match started {
                Ok(c) => {
                    self.native_host.client = Some(c);
                    lua_pushboolean(L, 1);
                    1
                }
                Err(e) => nil_err(L, &format!("native client: {e}")),
            }
        }
    }
    pub unsafe fn host_directory(&mut self, L: *mut lua_State) -> c_int {
        unsafe {
            let Some(d)=self.native_host.directory() else{lua_pushnil(L);return 1;};
            lua_createtable(L,0,8);let t=lua_gettop(L);
            set_int(L,t,"epoch",d.epoch as i64);set_int(L,t,"seq",d.seq as i64);set_int(L,t,"state",d.state as i64);
            set_str(L,t,"arena",&d.arena);set_str(L,t,"error",&d.error);
            if let Some(c)=self.native_host.client.as_ref(){set_int(L,t,"peer_id",c.peer_id() as i64);set_bool(L,t,"connected",c.connected());}
            lua_createtable(L,d.entities.len() as c_int,0);let rows=lua_gettop(L);
            for (i,e) in d.entities.iter().enumerate(){lua_createtable(L,0,9);let row=lua_gettop(L);push_ref(L,row,e.reference);set_int(L,row,"owner_peer",e.owner_peer as i64);set_int(L,row,"slot",e.slot as i64);set_int(L,row,"kind",e.kind as i64);set_int(L,row,"controller",e.controller as i64);set_bool(L,row,"team_known",e.team.is_some());if let Some(team)=e.team{set_int(L,row,"team",team as i64);}lua_rawseti(L,rows,i as i64+1);}
            rawset_str(L,t,"entities");1
        }
    }
    pub unsafe fn host_inputs(&mut self, L: *mut lua_State) -> c_int {
        unsafe {
            let Some(h)=self.native_host.host.as_ref() else{return nil_err(L,"not a native host");};
            let Some(max)=opt_int(L,1,32).filter(|n|*n>=0&&*n<=32) else{return nil_err(L,"input bound");};
            let inputs=h.inputs(max as usize);lua_createtable(L,inputs.len() as c_int,0);let out=lua_gettop(L);
            for (idx,i) in inputs.iter().enumerate(){lua_createtable(L,0,9);let row=lua_gettop(L);push_ref(L,row,i.reference);set_int(L,row,"seq",i.seq as i64);set_int(L,row,"delivery_seq",i.delivery_seq as i64);set_int(L,row,"buttons",i.buttons as i64);set_int(L,row,"flags",i.flags as i64);set_int(L,row,"sample_ms",i.sample_ms as i64);lua_createtable(L,8,0);let axes=lua_gettop(L);fill_array(L,axes,i.axes.iter().map(|x|*x as f64));rawset_str(L,row,"axes");lua_rawseti(L,out,idx as i64+1);}1
        }
    }
    pub unsafe fn host_status(&mut self, L: *mut lua_State) -> c_int {
        unsafe {
            let Some(h)=self.native_host.host.as_ref() else{return nil_err(L,"not a native host");};
            let Some(state)=arg_str(L,1) else{return nil_err(L,"host state");};
            let code=match state{"native_ready"=>w::READY,"live"=>w::LIVE,"victory"=>w::VICTORY,"defeat"=>w::DEFEAT,"error"=>w::FAULT,"boot"|"travel"|"wait_world"|"native_spawn"|"stopped"=>w::BOOTING,_=>return nil_err(L,"host state")};
            let error=arg_str(L,2).unwrap_or("");match h.status(code,error){Ok(())=>{lua_pushboolean(L,1);1},Err(e)=>nil_err(L,e)}
        }
    }
    pub unsafe fn native_input(&mut self, L: *mut lua_State) -> c_int {
        unsafe {
            let Some(c)=self.native_host.client.as_ref() else{return nil_err(L,"not a native client");};
            if !is_table(L,1){return nil_err(L,"input table");}let t=lua_absindex(L,1);
            let Some(epoch)=field_int(L,t,"epoch").filter(|x|*x!=0) else{return nil_err(L,"input epoch");};
            let uint=|L,key|field_int(L,t,key).and_then(|x|u32::try_from(x).ok());
            let (Some(id),Some(incarnation),Some(seq),Some(buttons))=(uint(L,"id"),uint(L,"incarnation"),uint(L,"seq"),uint(L,"buttons")) else{return nil_err(L,"input fields");};
            rawget_str(L,t,"axes");if !is_table(L,-1)||lua_rawlen(L,-1)!=8{pop(L,1);return nil_err(L,"input axes");}let axes_table=lua_absindex(L,-1);let mut axes=[0f32;8];
            for (i,x) in axes.iter_mut().enumerate(){let Some(v)=geti_num(L,axes_table,i as i64+1) else{pop(L,1);return nil_err(L,"input axes");};*x=v as f32;}pop(L,1);
            let frame=InputFrame{reference:EntityRef{epoch:epoch as u64,id,incarnation},seq,buttons,axes,..Default::default()};
            match c.input(frame){Ok(())=>{lua_pushboolean(L,1);1},Err(e)=>nil_err(L,e)}
        }
    }
    pub unsafe fn native_snapshot(&mut self,L:*mut lua_State)->c_int {
        unsafe {
            let Some(c)=self.native_host.client.as_ref() else{return nil_err(L,"not a native client");};
            let Some(w)=c.snapshot() else{lua_pushnil(L);return 1;};
            lua_createtable(L,0,4);let t=lua_gettop(L);set_int(L,t,"epoch",w.epoch as i64);set_int(L,t,"dir_seq",w.directory_seq as i64);set_int(L,t,"frame_seq",w.frame_seq as i64);
            lua_createtable(L,w.entities.len() as c_int,0);let rows=lua_gettop(L);
            for (i,e) in w.entities.iter().enumerate(){lua_createtable(L,0,6);let row=lua_gettop(L);push_ref(L,row,e.reference);
                let root=hsmp_ipc::schema::record_info(hsmp_ipc::schema::pose::K_ROOT).expect("root schema");crate::marshal::record_to_table(L,root,bytemuck::bytes_of(&e.root),None);rawset_str(L,row,"root");
                let vitals=hsmp_ipc::schema::record_info(hsmp_ipc::schema::combat::K_VITALS).expect("vitals schema");crate::marshal::record_to_table(L,vitals,bytemuck::bytes_of(&e.vitals),None);rawset_str(L,row,"vitals");
                push_decoded_pose(L,row,&e.pose,e.root.tick);lua_rawseti(L,rows,i as i64+1);
            }rawset_str(L,t,"entities");1
        }
    }
}
