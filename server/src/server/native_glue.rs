//! Native authority session. Player connection identity remains separate from native entities.
use super::*;
use crate::native_wire::{self as w, Directory, Entity, EntityRef, InputFrame};
use crate::native_service::Bridge;

struct InputState { seq: u32, delivery_seq: u32, last_ms: u64, held: bool, window_ms: u64, count: u32 }
pub(super) struct NativeCore {
    bridge: Arc<Bridge>, directory: Directory, input: HashMap<u32, InputState>,
    dirty: bool, frame_seq: u32, last_world_ms: u64,
}
impl NativeCore {
    pub fn new(bridge: Arc<Bridge>, epoch: u64, arena: &str) -> Self {
        let entities = (0..3).map(|slot| Entity { reference: EntityRef { epoch, id: slot + 1, incarnation: 1 }, owner_peer: 0,
            slot: slot as u16, kind: if slot == 2 { w::AI } else { w::HUMAN }, controller: if slot == 2 { 255 } else { slot as u8 }, team: if slot == 2 { 2 } else { 1 } }).collect();
        let directory = Directory { epoch, seq: 1, state: w::BOOTING, arena: arena.to_owned(), error: String::new(), entities };
        bridge.set_directory(directory.clone());
        Self { bridge, directory, input: HashMap::new(), dirty: true, frame_seq: 0, last_world_ms: 0 }
    }
    fn changed(&mut self) {
        if let Some(seq) = self.directory.seq.checked_add(1) { self.directory.seq = seq; }
        else { self.directory.state = w::FAULT; self.directory.error = "native directory counter exhausted".into(); }
        self.dirty = true; self.frame_seq = 0; self.bridge.set_directory(self.directory.clone());
    }
    fn release(&mut self, id: u32) {
        let Some(s) = self.input.get_mut(&id) else { return };
        let Some(e) = self.directory.entities.iter().find(|e| e.reference.id == id) else { return };
        if !s.held { return; }
        let Some(seq) = s.delivery_seq.checked_add(1) else { self.directory.state = w::FAULT; return };
        let i = InputFrame { reference: e.reference, seq: s.seq.max(1), delivery_seq: seq, flags: w::RELEASE_ALL, ..Default::default() };
        if !self.bridge.push_input(i) { self.bridge.clear_inputs(); let _ = self.bridge.push_input(i); }
        s.held = false; s.delivery_seq = seq;
    }
    fn bind(&mut self, peer: u32, resume: bool) {
        let current = self.directory.entities.iter().position(|e| e.owner_peer == peer);
        if current.is_some() && !resume { return; }
        let slot = current.or_else(|| self.directory.entities.iter().position(|e| e.kind == w::HUMAN && e.owner_peer == 0));
        let Some(slot) = slot else { return };
        let id = self.directory.entities[slot].reference.id; self.release(id); self.input.remove(&id);
        let e = &mut self.directory.entities[slot];
        let Some(next) = e.reference.incarnation.checked_add(1) else { self.directory.state = w::FAULT; self.changed(); return };
        e.reference.incarnation = next; e.owner_peer = peer; self.changed();
    }
    fn unbind(&mut self, peer: u32) {
        let Some(slot) = self.directory.entities.iter().position(|e| e.owner_peer == peer) else { return };
        let id = self.directory.entities[slot].reference.id; self.release(id); self.input.remove(&id);
        self.directory.entities[slot].owner_peer = 0;
        if let Some(next) = self.directory.entities[slot].reference.incarnation.checked_add(1) { self.directory.entities[slot].reference.incarnation = next; }
        else { self.directory.state = w::FAULT; }
        if self.directory.state == w::LIVE { self.directory.state = w::READY; }
        self.changed();
    }
}
pub(super) fn joined(inner: &mut Inner, peer: u32, resume: bool) { if let Some(n) = inner.native.as_mut() { n.bind(peer, resume); } }
pub(super) fn left(inner: &mut Inner, peer: u32) { if let Some(n) = inner.native.as_mut() { n.unbind(peer); } }

pub(super) async fn handle(state: &Arc<ServerState>, from: SocketAddr, kind: u16, payload: &[u8]) -> anyhow::Result<()> {
    // Only input can be supplied by a player. Pose, vitals, directory and world are local-source-only.
    if kind != w::K_INPUT { anyhow::bail!("native client cannot publish authority"); }
    let mut i = w::decode_input(payload).map_err(anyhow::Error::msg)?;
    if i.flags != 0 || i.delivery_seq != 0 { anyhow::bail!("native client input flags"); }
    let now = state.net.now_ms(); let mut inner = state.inner.lock().await;
    if now.saturating_sub(i.sample_ms) >= w::INPUT_TIMEOUT_MS || i.sample_ms > now.saturating_add(100) { anyhow::bail!("stale native input capture"); }
    let peer = inner.peers.get(&from).ok_or_else(|| anyhow::anyhow!("native input has no authenticated player"))?.id;
    let Some(n) = inner.native.as_mut() else { return Ok(()) };
    if n.bridge.reset_pending(){anyhow::bail!("native world reset pending");}
    if !matches!(n.directory.state, w::READY | w::LIVE) { return Ok(()) }
    if !n.directory.entities.iter().any(|e| e.reference == i.reference && e.owner_peer == peer && e.kind == w::HUMAN) { anyhow::bail!("native input ownership"); }
    let s = n.input.entry(i.reference.id).or_insert(InputState { seq: 0, delivery_seq: 0, last_ms: now, held: false, window_ms: now, count: 0 });
    if i.seq <= s.seq { return Ok(()) }
    if now.saturating_sub(s.window_ms) >= 1000 { s.window_ms = now; s.count = 0; }
    if s.count >= 120 { anyhow::bail!("native input rate"); }
    let seq = s.delivery_seq.checked_add(1).ok_or_else(|| anyhow::anyhow!("native input counter exhausted"))?;
    i.delivery_seq = seq;
    if !n.bridge.push_input(i) { anyhow::bail!("native game input queue full"); }
    s.seq = i.seq; s.delivery_seq = seq; s.count += 1; s.last_ms = now; s.held = true; Ok(())
}

pub(super) fn tick(inner: &mut Inner, now: u64) {
    inner.now_ms = now.max(inner.now_ms);
    let Some(mut n) = inner.native.take() else { return };
    if n.bridge.take_world_reset(){
        n.input.clear();n.last_world_ms=0;n.directory.state=w::BOOTING;n.directory.error.clear();
        for e in &mut n.directory.entities{
            if let Some(next)=e.reference.incarnation.checked_add(1){e.reference.incarnation=next;}
            else{n.directory.state=w::FAULT;n.directory.error="native actor incarnation exhausted".into();}
        }
        n.changed();
    }
    if let Some((status, error)) = n.bridge.take_status() {
        if n.directory.state != status || n.directory.error != error { n.directory.state = status; n.directory.error = error; n.changed(); }
    }
    // A vanished game-thread producer stops all held inputs; it cannot leave a fighting world live.
    let expired: Vec<u32> = n.input.iter().filter(|(_, s)| s.held && now.saturating_sub(s.last_ms) >= w::INPUT_TIMEOUT_MS).map(|(id, _)| *id).collect();
    for id in expired { n.release(id); }
    if let Some(world) = n.bridge.take_publication() {
        if w::matches_directory(&world, &n.directory) && world.frame_seq > n.frame_seq {
            n.frame_seq = world.frame_seq; n.last_world_ms = now;
            for snapshot in &world.entities {
                if let Some(entity) = n.directory.entities.iter().find(|e| e.reference == snapshot.reference) {
                    if let Some(p) = inner.peers.values_mut().find(|p| p.id == entity.owner_peer) {
                        p.alive = snapshot.vitals.flags & hsmp_ipc::schema::combat::VF_DEAD == 0;
                        p.last_root = Some(snapshot.root); p.last_valid_pos = Some(snapshot.root.pos); p.last_valid_ms = now;
                    }
                }
            }
            let all_connected = n.directory.entities.iter().filter(|e| e.kind == w::HUMAN).all(|e| e.owner_peer != 0);
            if all_connected && matches!(n.directory.state, w::READY | w::LIVE) {
                let alive = |kind| world.entities.iter().any(|s| s.vitals.flags & hsmp_ipc::schema::combat::VF_DEAD == 0 && n.directory.entities.iter().any(|e| e.reference == s.reference && e.kind == kind));
                let status = if !alive(w::HUMAN) { w::DEFEAT } else if !alive(w::AI) { w::VICTORY } else { w::LIVE };
                if n.directory.state != status { n.directory.state = status; n.dirty = true; n.bridge.set_directory(n.directory.clone()); }
            }
            if let Ok(payload) = w::encode_world(&world) { inner.out_msgs.push((None, hsmp_ipc::wire::message(w::K_WORLD, 0, 0, &payload))); }
        }
    }
    if n.last_world_ms != 0 && now.saturating_sub(n.last_world_ms) > 2000 && n.directory.state == w::LIVE {
        n.directory.state = w::FAULT; n.directory.error = "native simulation stopped publishing".into();
        let ids: Vec<u32> = n.input.keys().copied().collect(); for id in ids { n.release(id); } n.changed();
    }
    if n.dirty {
        n.dirty = false;
        if let Ok(payload) = w::encode_directory(&n.directory) { inner.out_msgs.push((None, hsmp_ipc::wire::message(w::K_DIRECTORY, 0, 0, &payload))); }
    }
    // Legacy match decisions never run against human-only standing counts in native co-op.
    inner.match_state = match n.directory.state { w::LIVE => "live", w::VICTORY | w::DEFEAT => "match_over", _ => "lobby" }.into();
    inner.native = Some(n);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn authenticated_sender_cannot_spoof_entity_world_or_generation() {
        let bridge=Arc::new(Bridge::default());
        let state=Arc::new(ServerState::with_native(2,crate::net::Net::ephemeral(),bridge.clone(),"Map_Arena_Yard"));
        let a:SocketAddr="127.0.0.1:10021".parse().unwrap();let b:SocketAddr="127.0.0.1:10022".parse().unwrap();
        let (first,second)={let mut inner=state.inner.lock().await;
            inner.peers.insert(a,match_core::round_tests::peer(71001,"a"));inner.peers.insert(b,match_core::round_tests::peer(71002,"b"));
            joined(&mut inner,71001,false);joined(&mut inner,71002,false);
            let n=inner.native.as_mut().unwrap();n.directory.state=w::READY;
            (n.directory.entities[0].reference,n.directory.entities[1].reference)
        };
        let input=InputFrame{reference:first,seq:1,sample_ms:state.net.now_ms(),buttons:1,..Default::default()};
        let payload=w::encode_input(&input).unwrap();
        assert!(handle(&state,b,w::K_INPUT,&payload).await.is_err());
        assert!(handle(&state,a,w::K_WORLD,&payload).await.is_err());
        assert!(handle(&state,a,w::K_DIRECTORY,&payload).await.is_err());
        handle(&state,a,w::K_INPUT,&payload).await.unwrap();
        assert_eq!(bridge.take_inputs(32)[0].reference,first);
        handle(&state,a,w::K_INPUT,&payload).await.unwrap();assert!(bridge.take_inputs(32).is_empty());
        {let mut inner=state.inner.lock().await;joined(&mut inner,71001,true);}
        assert!(handle(&state,a,w::K_INPUT,&payload).await.is_err());
        let new_ref=state.inner.lock().await.native.as_ref().unwrap().directory.entities[0].reference;
        assert_eq!(new_ref.id,first.id);assert!(new_ref.incarnation>first.incarnation);
        assert!(handle(&state,a,w::K_INPUT,&w::encode_input(&InputFrame{reference:EntityRef{epoch:new_ref.epoch.wrapping_add(1),..new_ref},seq:2,sample_ms:state.net.now_ms(),..Default::default()}).unwrap()).await.is_err());
        assert!(handle(&state,a,w::K_INPUT,&w::encode_input(&InputFrame{reference:second,seq:2,sample_ms:state.net.now_ms(),..Default::default()}).unwrap()).await.is_err());
        bridge.request_world_reset();
        assert!(!bridge.push_input(InputFrame{reference:new_ref,seq:2,..Default::default()}));
        assert!(handle(&state,a,w::K_INPUT,&payload).await.is_err());
        let after_reset={let mut inner=state.inner.lock().await;tick(&mut inner,state.net.now_ms());let n=inner.native.as_mut().unwrap();n.directory.state=w::READY;n.directory.entities[0].reference};
        assert!(after_reset.incarnation>new_ref.incarnation);
        assert!(handle(&state,a,w::K_INPUT,&w::encode_input(&InputFrame{reference:new_ref,seq:2,sample_ms:state.net.now_ms(),..Default::default()}).unwrap()).await.is_err());
        assert!(bridge.take_inputs(32).is_empty());
    }
    #[test]
    fn detached_worker_never_runs_human_only_round_decisions() {
        let bridge=Arc::new(Bridge::default());let mut core=NativeCore::new(bridge,99,"Map_Arena_Yard");
        core.bind(71011,false);core.bind(71012,false);
        assert_eq!(core.directory.entities.iter().filter(|e|e.kind==w::AI).count(),1);
        assert_eq!(core.directory.state,w::BOOTING);
        core.unbind(71011);assert_eq!(core.directory.entities[0].owner_peer,0);
    }
}
