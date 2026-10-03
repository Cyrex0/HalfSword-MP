//! DevCtl: `hsmp-tools ipc-ctl` pushes `dev_cmd` records into the segment's
//! DevCtl ring (any producer epoch). Several mods consume them, each in its own Lua state,
//! while the ring is SPSC: the native module drains it (at `frame()` and at `dev_poll`)
//! into a small in-process log with one cursor per Lua state (registered when the state gets
//! the API, starting at "now"), so every state sees every command once, in ring order.

use std::collections::HashMap;
use std::ffi::c_int;

use hsmp_ipc::header::ctr;
use hsmp_ipc::ring::{Pop, Record};
use hsmp_ipc::schema::dev::K_DEV_CMD;
use hsmp_ipc::schema::{self as sc};

use crate::lua::*;
use crate::marshal;
use crate::native::Native;

/// In-process DevCtl log size.
pub const DEV_LOG: usize = 256;

#[derive(Default)]
pub struct DevState {
    log: Vec<Record>,
    head: u64,
    cursors: HashMap<usize, u64>,
}

impl DevState {
    /// A Lua state got the API: its cursor starts at "now".
    pub fn register(&mut self, main: usize) {
        self.cursors.insert(main, self.head);
    }
}

impl Native {
    /// Move every pending DevCtl record into the dev log (invalid ones are dropped).
    pub(crate) fn dev_drain(&mut self) {
        let Some(s) = self.seg() else { return };
        let ring = s.devctl();
        if ring.is_empty() {
            return;
        }
        if self.dev.log.is_empty() {
            self.dev.log = vec![Record::default(); DEV_LOG];
        }
        let Some(info) = sc::record_info(K_DEV_CMD) else { return };
        let mut rec = Record::default();
        for _ in 0..(2 * DEV_LOG) {
            match ring.pop(0, &mut rec) {
                Pop::Empty => break,
                Pop::Record => {
                    if rec.hdr.kind != K_DEV_CMD || (info.check)(rec.payload()).is_err() {
                        s.header.game.count(ctr::BAD_RECORD, 1);
                        continue;
                    }
                    let i = (self.dev.head % DEV_LOG as u64) as usize;
                    self.dev.log[i] = rec;
                    self.dev.head += 1;
                }
                Pop::StaleEpoch => {}
                Pop::Bad => s.header.game.count(ctr::BAD_RECORD, 1),
                Pop::Resynced => s.header.game.count(ctr::RESYNC, 1),
            }
        }
    }

    /// `dev_poll(max, out)` -> `n`; `out[i] = {kind = "dev_cmd", data = t}` (entry tables
    /// reused, `data` fresh), trailing entries cleared.
    pub unsafe fn dev_poll(&mut self, L: *mut lua_State) -> c_int {
        unsafe {
            let (Some(max), true) = (opt_int(L, 1, 16), is_table(L, 2)) else { return nil_err(L, "bad") };
            let out = lua_absindex(L, 2);
            self.dev_drain();
            let main = main_thread(L);
            let head = self.dev.head;
            let oldest = head.saturating_sub(DEV_LOG as u64);
            let mut cur = self.dev.cursors.get(&main).copied().unwrap_or(head);
            if cur < oldest {
                self.count_overflow();
                cur = oldest;
            }
            let info = sc::record_info(K_DEV_CMD);
            let max = max.clamp(0, DEV_LOG as i64);
            let mut n: i64 = 0;
            while n < max && cur < head {
                let rec = &self.dev.log[(cur % DEV_LOG as u64) as usize];
                cur += 1;
                let Some(info) = info else { break };
                n += 1;
                if lua_rawgeti(L, out, n) != LUA_TTABLE {
                    pop(L, 1);
                    lua_createtable(L, 0, 2);
                    lua_pushvalue(L, -1);
                    lua_rawseti(L, out, n);
                }
                let t = lua_gettop(L);
                set_str(L, t, "kind", info.name);
                if marshal::record_to_table(L, info, rec.payload(), None) {
                    rawset_str(L, t, "data");
                } else {
                    set_nil(L, t, "data");
                }
                pop(L, 1);
            }
            self.dev.cursors.insert(main, cur);
            trim_array(L, out, n);
            lua_pushinteger(L, n);
            1
        }
    }

    fn count_overflow(&self) {
        if let Some(s) = self.seg() {
            s.header.game.count(ctr::OVERFLOW, 1);
        }
    }
}
