#!/usr/bin/env bash
# e2e-test.sh — exercise every subsystem of the HSMP stack and report
# PASS/FAIL for each. Runs the real binaries over real UDP + HTTP; the
# only piece not exercised is the in-game Lua execution (UE4SS runtime
# can't be driven from outside the game process). Lua files are
# lint-checked with luac if available.
#
# Isolation: every port is allocated per run (a free port from
# 20000-48999, reserved with an atomic lock dir so concurrent runs never pick
# the same one, released at exit), servers are waited for by their bound port
# (not a fixed sleep), and the scratch dir is unique per process:
# ${TMPDIR:-/tmp}/hsmp-e2e-<pid>-<stamp> (HSMP_SCRATCH overrides).
#
#   HSMP_REPO   the tree to test        (default: this script's checkout)
#   HSMP_BINS   the server release dir  (default: $CARGO_TARGET_DIR/release, else <repo>/target/release)
#   HSMP_TOOLS  the hsmp-tools.exe file (default: $CARGO_TARGET_DIR/release/hsmp-tools.exe, else
#               <repo>/target/release/hsmp-tools.exe). Never built here.
#   IPC         shared memory only. Every sidecar runs behind a fake game
#               (`hsmp-tools ipc-game`, see sc_launch); S1-S12 (scripts/e2e-shm.sh) check
#               the shared-memory contracts directly.
# Returns 0 if everything passes, 1 otherwise.

set -u -o pipefail

e2e_unix_path() { if command -v cygpath >/dev/null 2>&1; then cygpath -u "$1"; else printf '%s' "$1"; fi; }
REPO="${HSMP_REPO:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
E2E_TARGET="${CARGO_TARGET_DIR:+$(e2e_unix_path "$CARGO_TARGET_DIR")}"
# HSMP_BINS lets a parallel build dir be tested (e.g. another worktree's target/release).
BINS="${HSMP_BINS:-${E2E_TARGET:-$REPO/target}/release}"
# Every sidecar started here skips the career save guard (it would back up
# and restore the real %LOCALAPPDATA% SaveGames); the guard is ON by default.
export HSMP_CAREER_GUARD=0
MODS_SRC="$REPO/mods"
# T15 reads (never writes) the deployed game: HSMP_GAME_DIR, else <repo>/game, else the main checkout's game/
E2E_GAME="${HSMP_GAME_DIR:-}"
if [[ -z "$E2E_GAME" ]]; then
  E2E_GAME="$REPO/game"
  if [[ ! -d "$E2E_GAME" ]]; then
    E2E_COMMON=$(git -C "$REPO" rev-parse --path-format=absolute --git-common-dir 2>/dev/null)
    [[ -n "$E2E_COMMON" ]] && E2E_GAME="$(dirname "$E2E_COMMON")/game"
  fi
fi
GAME_MODS="$E2E_GAME/HalfswordUE5/Binaries/Win64/ue4ss/Mods"
# Rust dev tools (no Python): RCON client etc. Not built here (a build would land in the
# shared tree, or race another worktree's): build it first into your own target dir.
HSMP_TOOLS="${HSMP_TOOLS:-${E2E_TARGET:-$REPO/target}/release/hsmp-tools.exe}"
if [[ ! -x "$HSMP_TOOLS" ]]; then
  echo "hsmp-tools not found at $HSMP_TOOLS: build it (cargo build --release -p hsmp-tools" \
       "with your own CARGO_TARGET_DIR) and set HSMP_TOOLS to the .exe" >&2
  exit 1
fi

# Windows pid of this shell: every long-lived child gets --parent-pid, so a killed run leaves no orphans
E2E_WINPID=$(cat /proc/$$/winpid 2>/dev/null || echo $$)
STAMP="$(date +%s)"
SCRATCH="${HSMP_SCRATCH:-${TMPDIR:-/tmp}/hsmp-e2e-$$-$STAMP}"
mkdir -p "${SCRATCH:?}"
# Player and server identities: servers keep their key here (never the real
# %LOCALAPPDATA%\HSMP), and every sidecar launch below sets its own
# HSMP_STATE_DIR so each test client is a distinct player.
export HSMP_STATE_DIR="$SCRATCH/home"
mkdir -p "$HSMP_STATE_DIR"

PASS_COUNT=0
FAIL_COUNT=0
FAIL_NAMES=()

pass() { PASS_COUNT=$((PASS_COUNT+1)); printf '  \e[32mPASS\e[0m  %s\n' "$1"; }
fail() { FAIL_COUNT=$((FAIL_COUNT+1)); FAIL_NAMES+=("$1"); printf '  \e[31mFAIL\e[0m  %s\n' "$1" >&2; printf '        %s\n' "${2:-}" >&2; }
hdr()  { printf '\n\e[36m== %s ==\e[0m\n' "$1"; }

# --- per-run ports --------------------------------------------------------------------
# e2e_port VAR: a port free for TCP and UDP on this box, reserved for this run. Reservation
# is an atomic mkdir under a lock dir shared by every e2e run (owner pid inside; a lock
# whose owner is gone is reclaimed), so two concurrent runs never get the same port; the
# OS check (netstat) skips ports any other process holds; Windows' excluded port ranges
# are skipped. 20000-48999 stays below the ephemeral range (49152+) and away from the
# game gate's fixed ports (7777, 7778, 7790+, 27015).
E2E_PORT_LOCKS="${TMPDIR:-/tmp}/hsmp-e2e-ports"
E2E_MY_PORTS=()
E2E_EXCLUDED=""
e2e_excluded_ranges() {
  [[ -n "$E2E_EXCLUDED" ]] && return
  E2E_EXCLUDED="27015-27015"
  local proto
  for proto in tcp udp; do
    E2E_EXCLUDED+=" $(netsh int ipv4 show excludedportrange protocol=$proto 2>/dev/null | awk '$1 ~ /^[0-9]+$/ && $2 ~ /^[0-9]+$/ {printf "%s-%s ", $1, $2}')"
  done
}
e2e_port_excluded() {
  local p="$1" r
  for r in $E2E_EXCLUDED; do (( p >= ${r%-*} && p <= ${r#*-} )) && return 0; done
  return 1
}
e2e_port_in_use() {   # any local socket (TCP or UDP, any address) on this port
  netstat -ano 2>/dev/null | awk -v p="$1" '($1=="TCP"||$1=="UDP") { n=split($2,a,":"); if (a[n]==p) { found=1 } } END { exit !found }'
}
e2e_port() {
  local __var="$1" p tries=0 owner
  e2e_excluded_ranges
  mkdir -p "$E2E_PORT_LOCKS"
  while (( tries < 400 )); do
    tries=$((tries+1))
    p=$(( 20000 + ( (RANDOM << 15) | RANDOM ) % 29000 ))
    e2e_port_excluded "$p" && continue
    if ! mkdir "$E2E_PORT_LOCKS/$p" 2>/dev/null; then
      owner=$(cat "$E2E_PORT_LOCKS/$p/owner" 2>/dev/null)
      # reclaim a lock left by a run that died (no owner, or the owner shell is gone)
      if [[ -n "$owner" ]] && ! kill -0 "$owner" 2>/dev/null; then rm -rf "${E2E_PORT_LOCKS:?}/$p"; fi
      continue
    fi
    echo $$ > "$E2E_PORT_LOCKS/$p/owner"
    if e2e_port_in_use "$p"; then rm -rf "${E2E_PORT_LOCKS:?}/$p"; continue; fi
    E2E_MY_PORTS+=("$p")
    printf -v "$__var" '%s' "$p"
    return 0
  done
  echo "e2e_port: no free port after $tries tries" >&2
  return 1
}
e2e_release_ports() {
  local p
  for p in "${E2E_MY_PORTS[@]}"; do
    [[ "$(cat "$E2E_PORT_LOCKS/$p/owner" 2>/dev/null)" == "$$" ]] && rm -rf "${E2E_PORT_LOCKS:?}/$p"
  done
  E2E_MY_PORTS=()
}
# e2e_wait_bound PORT [PID] [TIMEOUT_S]: wait until 127.0.0.1:PORT is bound (UDP, or TCP
# LISTENING) instead of a fixed sleep. Fails early when PID (if given) exited.
e2e_wait_bound() {
  local port="$1" pid="${2:-}" t="${3:-15}" i=0
  while (( i < t * 10 )); do
    if netstat -ano 2>/dev/null | awk -v a="127.0.0.1:$port" '$2==a && ($1=="UDP" || $4=="LISTENING") { f=1 } END { exit !f }'; then
      return 0
    fi
    if [[ -n "$pid" ]] && ! kill -0 "$pid" 2>/dev/null; then
      echo "  (pid $pid exited before binding 127.0.0.1:$port)" >&2; return 1
    fi
    sleep 0.1; i=$((i+1))
  done
  echo "  (127.0.0.1:$port not bound after ${t}s)" >&2
  return 1
}
# e2e_wait_file TIMEOUT_S FILE REGEX: wait until FILE matches REGEX
e2e_wait_file() {
  local t="$1" f="$2" re="$3" i=0
  while (( i < t * 10 )); do grep -qE "$re" "$f" 2>/dev/null && return 0; sleep 0.1; i=$((i+1)); done
  return 1
}

cleanup() {
  local pids
  pids=$(jobs -p 2>/dev/null)
  if [[ -n "$pids" ]]; then
    kill $pids 2>/dev/null
    wait 2>/dev/null
  fi
  e2e_release_ports
}
trap cleanup EXIT

# sc_launch <state dir> <sidecar args...>: the REAL sidecar behind a fake game
# (`hsmp-tools ipc-game`). The sidecar talks to the game over shared memory
# only: ipc-game creates the segment, starts the sidecar with
# `--parent-pid <its pid> --ipc shm:<name> --ipc-tap <dir>/ipc_tap.jsonl`, logs what the
# game sees to <dir>/view.jsonl and, with --bridge, plays the part of the Lua facade: it
# mirrors the session / state / events the GAME receives into the legacy-shaped files in
# <dir> and forwards the queue / request / game-state files a test writes there. The
# sidecar never touches those files. The session domain (status, session snapshot, admin,
# commands, chat, leave) is typed records only: scripts/e2e-records.sh reads them from
# view.jsonl and writes them with ipc-put (the mapping name is in <dir>/ipc_name.txt).
# A `--parent-pid P` in the args becomes ipc-game's parent; `--status-file F` is dropped
# (the status is the `link` record). The identity
# dir is <dir> unless SC_IDDIR is set. $! of `sc_launch ... &` is the fake game, which
# exits with the sidecar's exit code.
sc_launch() {
  local d="$1"; shift
  local pp=() args=()
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --parent-pid) pp=(--parent-pid "$2"); shift 2 ;;
      --status-file) shift 2 ;;
      *) args+=("$1"); shift ;;
    esac
  done
  # a fresh view per launch: the record helpers read the newest lines of THIS run
  rm -f "$d/ipc_name.txt" "$d/view.jsonl"
  HSMP_STATE_DIR="${SC_IDDIR:-$d}" exec "$HSMP_TOOLS" ipc-game "${pp[@]}" --bridge "$d" --name-file "$d/ipc_name.txt" \
    --view "$d/view.jsonl" --tap "$d/ipc_tap.jsonl" -- "$BINS/hsmp-sidecar.exe" "${args[@]}"
}
# the typed-record helpers (link / session / admin records, cmd_result / chat_in events, ipc-put)
source "$REPO/scripts/e2e-records.sh"

# ============================================================================
hdr "T1: Rust binaries present and runnable"

for b in hsmp-server.exe hsmp-sidecar.exe hsmp-master.exe; do
  if [[ -x "$BINS/$b" ]]; then pass "$b exists ($(du -b "$BINS/$b" | cut -f1) bytes)"
  else fail "$b missing"; fi
done

if "$BINS/hsmp-server.exe" --help > "$SCRATCH/t1_server_help.txt" 2>&1; then
  pass "hsmp-server --help runs"
else fail "hsmp-server --help failed"; fi

if "$BINS/hsmp-sidecar.exe" --help > "$SCRATCH/t1_sidecar_help.txt" 2>&1; then
  pass "hsmp-sidecar --help runs"
else fail "hsmp-sidecar --help failed"; fi

if "$BINS/hsmp-master.exe" --help > "$SCRATCH/t1_master_help.txt" 2>&1; then
  pass "hsmp-master --help runs"
else fail "hsmp-master --help failed"; fi

# ============================================================================
hdr "T2: Lua mods syntax-lint"

# UE4SS runs Lua 5.4 (bitwise operators, //), so an older luac on PATH would
# reject valid mods. Only a 5.4 luac counts; G0's lua_check covers the rest.
if command -v luac >/dev/null 2>&1 && luac -v 2>&1 | grep -q "Lua 5\.4"; then
  for mod in "$MODS_SRC"/HSMP*/Scripts/main.lua "$MODS_SRC"/dev/HSMP*/Scripts/main.lua; do
    name=$(basename "$(dirname "$(dirname "$mod")")")
    if luac -p "$mod" 2>"$SCRATCH/lua_$name.err"; then
      pass "$name/main.lua syntax ok"
    else
      fail "$name/main.lua syntax" "$(cat $SCRATCH/lua_$name.err)"
    fi
  done
else
  # Fallback: byte-count every file and spot-check it's non-empty + starts
  # with a Lua-ish prefix. Not a real lint but catches gross corruption.
  for mod in "$MODS_SRC"/HSMP*/Scripts/main.lua "$MODS_SRC"/dev/HSMP*/Scripts/main.lua; do
    name=$(basename "$(dirname "$(dirname "$mod")")")
    size=$(wc -c <"$mod")
    first=$(head -1 "$mod")
    if [[ $size -gt 100 && "$first" == --* || "$first" == local* ]]; then
      pass "$name/main.lua present (${size}B, no luac — soft check)"
    else
      fail "$name/main.lua suspicious" "size=$size first=$first"
    fi
  done
fi

# ============================================================================
hdr "T3: Master registry — register, list, dashboard, heartbeat, delete"

e2e_port MPORT
"$BINS/hsmp-master.exe" --parent-pid "$E2E_WINPID" --bind "127.0.0.1:$MPORT" > "$SCRATCH/t3_master.log" 2>&1 &
MP=$!
e2e_wait_bound $MPORT $MP

# health
HEALTH=$(curl -s "http://127.0.0.1:$MPORT/v1/health")
if [[ "$HEALTH" == "ok" ]]; then pass "master /v1/health"
else fail "master /v1/health" "got: $HEALTH"; fi

# empty list
LIST0=$(curl -s "http://127.0.0.1:$MPORT/v1/servers")
if [[ "$LIST0" == "[]" ]]; then pass "master /v1/servers empty"
else fail "master /v1/servers empty" "got: $LIST0"; fi

# register
REG=$(curl -s -X POST "http://127.0.0.1:$MPORT/v1/register" \
  -H "content-type: application/json" \
  -d '{"name":"E2E Server","mode":"duel","host":"","port":7777,"players":1,"max_players":4,"password_required":false,"nonce":"e2e","hmac":""}')
SID=$(echo "$REG" | grep -o '"server_id":"[^"]*"' | cut -d'"' -f4)
SECRET=$(echo "$REG" | grep -o '"secret":"[^"]*"' | cut -d'"' -f4)
if [[ -n "$SID" && ${#SID} -eq 32 && -n "$SECRET" ]]; then
  pass "master /v1/register returned server_id+secret"
else
  fail "master /v1/register" "got: $REG"
fi

# list populated
LIST1=$(curl -s "http://127.0.0.1:$MPORT/v1/servers")
if echo "$LIST1" | grep -q '"name":"E2E Server"'; then pass "master /v1/servers shows registered"
else fail "list after register" "$LIST1"; fi

# dashboard HTML
DASH=$(curl -s "http://127.0.0.1:$MPORT/")
if echo "$DASH" | grep -q "E2E Server" && echo "$DASH" | grep -q "<title>"; then
  pass "master / dashboard renders"
else
  fail "dashboard content" "first 200: ${DASH:0:200}"
fi

# heartbeat
HB=$(curl -s -o /dev/null -w "%{http_code}" -X POST "http://127.0.0.1:$MPORT/v1/heartbeat/$SID" \
  -H "content-type: application/json" \
  -d "{\"players\":2,\"nonce\":\"hb1\",\"hmac\":\"$SECRET\"}")
if [[ "$HB" == "204" ]]; then pass "master /v1/heartbeat → 204"
else fail "heartbeat status" "got $HB"; fi

# delete — body must carry nonce + 64-hex hmac (shape-only check in current master)
DEL=$(curl -s -o /dev/null -w "%{http_code}" -X DELETE "http://127.0.0.1:$MPORT/v1/servers/$SID" \
  -H "content-type: application/json" \
  -d "{\"nonce\":\"del1\",\"hmac\":\"$SECRET\"}")
if [[ "$DEL" == "204" ]]; then pass "master DELETE /v1/servers/<id>"
else fail "delete status" "got $DEL"; fi

LIST2=$(curl -s "http://127.0.0.1:$MPORT/v1/servers")
if [[ "$LIST2" == "[]" ]]; then pass "master list empty after delete"
else fail "list after delete" "$LIST2"; fi

kill $MP 2>/dev/null
wait $MP 2>/dev/null

# ============================================================================
hdr "T4-T11: Two-peer encrypted stack (handshake, snapshots, chat, weapon, skel, match)"

e2e_port PORT
DIRA="$SCRATCH/sideA"; DIRB="$SCRATCH/sideB"
mkdir -p "${DIRA:?}" "${DIRB:?}"

# Security: A hosts (listen server: admin by its player key, never by joining first).
"$BINS/hsmp-server.exe" --parent-pid "$E2E_WINPID" --bind "127.0.0.1:$PORT" --tick-hz 30 --max-peers 4 --owner-key-file "$DIRA/.player_key" > "$SCRATCH/t4_sv.log" 2>&1 &
SV=$!
e2e_wait_bound $PORT $SV
sc_launch "$DIRA" --parent-pid "$E2E_WINPID" --server "127.0.0.1:$PORT" --state-dir "$DIRA" --nick "Alpha" > "$SCRATCH/t4_scA.log" 2>&1 &
SA=$!
# make A join first → peer 1 (back-to-back launches race): wait for its handshake
wait_link 10 "$DIRA" connected || sleep 0.6
sc_launch "$DIRB" --parent-pid "$E2E_WINPID" --server "127.0.0.1:$PORT" --state-dir "$DIRB" --nick "Bravo" > "$SCRATCH/t4_scB.log" 2>&1 &
SB=$!
wait_link 10 "$DIRB" connected || sleep 1.2

# T4: handshake (the sidecar's `link` record: status CONNECTED, my peer id)
if link_is "$DIRA" connected && [[ "$(link_get "$DIRA" my_peer_id)" == 1 ]]; then
  pass "T4 handshake A: peer_id=1 connected"
else fail "T4 handshake A" "$(link_show "$DIRA")"; fi
if link_is "$DIRB" connected && [[ "$(link_get "$DIRB" my_peer_id)" == 2 ]]; then
  pass "T4 handshake B: peer_id=2 connected"
else fail "T4 handshake B" "$(link_show "$DIRB")"; fi

# T5: encryption indicator — the encrypted handshake landed (player key fingerprint
# logged) and the sidecars saw an encrypted connection.
nocolor() { sed 's/\x1b\[[0-9;]*m//g' "$@"; }
if nocolor "$SCRATCH/t4_sv.log" | grep -q "peer joined.*player=" && \
   grep -q "v5 connected (encrypted)" "$SCRATCH/t4_scA.log" && \
   grep -q "v5 connected (encrypted)" "$SCRATCH/t4_scB.log"; then
  pass "T5 v5 encrypted handshake (player fingerprints logged)"
else fail "T5 encryption" "$(grep -E 'peer joined|v5' $SCRATCH/t4_sv.log | head -3)"; fi

# T6: snapshot forwarding — A's game root (me_*.json through the fake game -> the local_root
# record, built as HSMPNative builds it)
# reaches B's game as PeerRoot (shared memory; seen in B's game view).
echo '{"pid":9001,"nick":"Alpha","tick":42,"pos":[500.000,0.000,100.000],"rot":[0.000,0.000,0.000],"vel":[0.0,0.0,0.0]}' > "$DIRA/me_9001.json"
if e2e_wait_file 5 "$DIRB/view.jsonl" '"ev":"peer_root".*"peer":1.*"pos":\[500\.0,0\.0,100\.0\]'; then
  pass "T6 root-snapshot forwarding (A pos=500 reaches B's PeerRoot)"
else
  fail "T6 forwarding" "$(grep '"ev":"peer_root"' "$DIRB/view.jsonl" 2>/dev/null | tail -1 | head -c 300)"
fi

# T7: chat (a typed `chat` record from A's game -> the server -> B's game as `chat_in`)
send_chat "$DIRA" "e2e chat test"
if wait_chat 3 "$DIRB" "e2e chat test"; then
  pass "T7 chat broadcast (A → B)"
else
  fail "T7 chat" "B chat_in: $(s2g "$DIRB" chat_in | tail -2 | head -c 300)"
fi

# T8: weapon
echo '{"tick":100,"weapon_id":2,"pos":[11.000,22.000,33.000],"rot":[1.000,2.000,3.000],"vel":[0.0,0.0,0.0],"held":1}' > "$DIRA/.weapon.json"
sleep 1.2
# .weapon_remote<id>.json has no reader (the weapon rides in the v2 pose), so the sidecar
# does not write it.
if [[ ! -e "$DIRB/.weapon_remote1.json" ]]; then
  pass "T8 weapon sent; dead .weapon_remote contract not written (A0)"
else
  fail "T8 weapon" "dead contract .weapon_remote1.json written"
fi

# T9: skeletal
# Real game bone names (case-sensitive, shared HERO_BONES table) + sender ts.
cat > "$DIRA/.skeletal.json" <<'EOF'
{"tick":77,"ts":5000,"bones":{"Pelvis":[0.0,0.0,100.0,0.0,0.0,0.0,1.0],"Head":[0.0,0.0,180.0,0.1,0.2,0.3,0.9]}}
EOF
sleep 1.2
# The raw .skel_remote<id>.json diagnostics file has no reader, so it is not written; the
# played-back pose (the PeerPlay slot) is checked by T9b.
if [[ ! -e "$DIRB/.skel_remote1.json" ]]; then
  pass "T9 skeletal sent; dead .skel_remote contract not written (A0)"
else
  fail "T9 skeletal" "dead contract .skel_remote1.json written"
fi
# T9b: the receiver's jitter buffer publishes the played-back pose for HSMPAvatars
# (PeerPlay slot of peer 1 in B's segment; v1 sender bones land in their v2 slots).
if e2e_wait_file 5 "$DIRB/view.jsonl" '"ev":"peer_play".*"peer":1.*"pelvis":\[0\.0,0\.0,100\.0'; then
  pass "T9b pose playback (B's PeerPlay for peer 1)"
else
  fail "T9b pose playback" "$(grep '"ev":"peer_play"' "$DIRB/view.jsonl" 2>/dev/null | tail -1 | head -c 300)"
fi
# T9c: vitals — A's vitals record (quantised once by the game: v = round(x * 64), 65535 =
# unknown; dism bit 11 = lowerarm_l) reaches B's peer_vitals slot with every field, the
# flags and the severed part, as written (sidecar gate -> ledger -> relay -> slot).
rec_put "$DIRA" vitals '{"seq":1,"flags":2,"dism":2048,"v":[5600,6400,1600,3920,3840,65535,1936,6400,960,6400,6400,3424,5632,1504,768,112,96,2112,32]}'
if e2e_wait_file 5 "$DIRB/view.jsonl" '"ev":"slot".*"slot":"peer_vitals"'; then
  VR=$(grep '"ev":"slot".*"slot":"peer_vitals"' "$DIRB/view.jsonl" | tail -1)
  if echo "$VR" | grep -q '"v":\[5600,6400,1600,3920,3840,65535,1936,6400,960,6400,6400,3424,5632,1504,768,112,96,2112,32\]' \
     && echo "$VR" | grep -q '"flags":2' && echo "$VR" | grep -q '"dism":2048' && echo "$VR" | grep -q '"peer":1'; then
    pass "T9c vitals record relayed (limbs, stamina, flags, severed part)"
  else
    fail "T9c vitals record" "$(echo "$VR" | head -c 400)"
  fi
else
  fail "T9c vitals record" "no peer_vitals slot in B's view"
fi

# T10: match flow with ready gating. Typed commands with ids; the server answers each.
send_cmd "$DIRA" 1001 start
wait_cmd_result 3 "$DIRA" 1001
# Server should have rejected (not all ready) → phase stays lobby
if phase_is "$DIRA" lobby && cmd_ok "$DIRA" 1001 false; then
  pass "T11 ready gating: start-without-ready refused (state stays lobby)"
else
  fail "T11 ready gating" "$(sess_show "$DIRA") | $(cmd_result "$DIRA" 1001)"
fi
if [[ "$(cmd_reason "$DIRA" 1001)" == NOT_ALL_READY ]] && jget "$(cmd_result "$DIRA" 1001)" reason_text | grep -q "start blocked"; then
  pass "T11 ready gating: the refusal (NOT_ALL_READY, start blocked) delivered to the requester"
else
  fail "T11 ready gating result" "$(cmd_result "$DIRA" 1001)"
fi

send_ready "$DIRA" 1002
send_ready "$DIRB" 2002
wait_cmd_result 3 "$DIRA" 1002; wait_cmd_result 3 "$DIRB" 2002
send_cmd "$DIRA" 1003 start
if wait_phase 3 "$DIRA" 'loading|countdown'; then
  pass "T10 match flow: ready+start → countdown"
else
  fail "T10 countdown" "$(sess_show "$DIRA") | $(cmd_result "$DIRA" 1003)"
fi
# T10s: the server-authoritative spawn plan (docs/development/subsystems/spawns.md) rides in the session roster:
# both clients get the same orders for round 1, one per peer on distinct slots.
T10S_A=$(sess_rows "$DIRA" | grep -o '"spawn_id":[0-9]*,"spawn_pos":\[[^]]*\],"spawn_protect_ms":[0-9]*,"spawn_slot":[0-9]*' | sort)
T10S_B=$(sess_rows "$DIRB" | grep -o '"spawn_id":[0-9]*,"spawn_pos":\[[^]]*\],"spawn_protect_ms":[0-9]*,"spawn_slot":[0-9]*' | sort)
SP_N=0; for sid in $(echo "$T10S_A" | grep -o '"spawn_id":[0-9]*' | cut -d: -f2); do (( (sid >> 8) == 1 )) && SP_N=$((SP_N+1)); done
SP_SLOTS=$(echo "$T10S_A" | grep -o '"spawn_slot":[0-9]*' | sort -u | wc -l)
if [[ "$SP_N" -eq 2 && "$SP_SLOTS" -eq 2 && "$T10S_A" == "$T10S_B" ]]; then
  pass "T10s spawn plan: 2 distinct server spawns for round 1, identical on both clients"
else
  fail "T10s spawn plan" "A: $T10S_A | B: $T10S_B"
fi
if wait_phase 6 "$DIRA" live && [[ "$(sess_get "$DIRA" round)" == 1 ]]; then
  pass "T10 match flow: countdown → live round=1"
else
  fail "T10 live" "$(sess_show "$DIRA")"
fi

# T12: speed cap — fire a teleport
# (a root inside the world bound: the record check refuses one outside it before it leaves
# the game. A first root after a spawn is a baseline, so set one, then jump 900 m.)
echo '{"pid":9001,"nick":"Alpha","tick":43,"pos":[600.000,0.000,100.000],"rot":[0.000,0.000,0.000],"vel":[0.0,0.0,0.0]}' > "$DIRA/me_9001.json"
sleep 0.5
echo '{"pid":9001,"nick":"Alpha","tick":44,"pos":[90000.000,0.000,100.000],"rot":[0.000,0.000,0.000],"vel":[0.0,0.0,0.0]}' > "$DIRA/me_9001.json"
sleep 1.0
if grep -q "speed cap exceeded" "$SCRATCH/t4_sv.log"; then
  pass "T12 teleport speed cap caught the jump"
else
  fail "T12 speed cap" "no 'speed cap exceeded' in server log"
fi

# T5b: no AEAD failures on the server over the whole two-peer run (the
# transport stats line is logged every 10 s: wait for one, the phase above can be shorter).
e2e_wait_file 12 "$SCRATCH/t4_sv.log" "v5 transport stats" || true
T5_STATS=$(grep "v5 transport stats" "$SCRATCH/t4_sv.log" | sed 's/\x1b\[[0-9;]*m//g')
if [[ -n "$T5_STATS" ]] && ! echo "$T5_STATS" | grep -v -q "aead_failed=0 "; then
  pass "T5b no decrypt failures during normal ops"
else fail "T5b decrypt failures" "$(grep 'v5 transport stats' $SCRATCH/t4_sv.log | tail -1)"; fi

kill $SV $SA $SB 2>/dev/null
wait 2>/dev/null

# ============================================================================
hdr "T13: Sidecar reconnect (v5 idle timeout + re-handshake, same identity)"

e2e_port PORT2
"$BINS/hsmp-server.exe" --parent-pid "$E2E_WINPID" --bind "127.0.0.1:$PORT2" --tick-hz 30 > "$SCRATCH/t13_sv1.log" 2>&1 &
SV1=$!
e2e_wait_bound $PORT2 $SV1
DIRR="$SCRATCH/sideReconn"
mkdir -p "${DIRR:?}"
sc_launch "$DIRR" --parent-pid "$E2E_WINPID" --server "127.0.0.1:$PORT2" --state-dir "$DIRR" --nick "Reconnector" > "$SCRATCH/t13_sc.log" 2>&1 &
SC=$!
sleep 1

if grep -q "welcome from relay" "$SCRATCH/t13_sc.log"; then
  pass "T13a initial welcome landed"
else fail "T13a initial welcome"; fi

kill $SV1 2>/dev/null; wait $SV1 2>/dev/null
# The connection idles out after 10 s of silence, then the sidecar re-handshakes (1, 2, 4 s backoff).
sleep 14

if grep -q "connection lost" "$SCRATCH/t13_sc.log"; then
  pass "T13b idle timeout detected the outage"
else fail "T13b outage detect"; fi

"$BINS/hsmp-server.exe" --parent-pid "$E2E_WINPID" --bind "127.0.0.1:$PORT2" --tick-hz 30 > "$SCRATCH/t13_sv2.log" 2>&1 &
SV2=$!
e2e_wait_bound $PORT2 $SV2
sleep 4

WELCOMES=$(grep -c "welcome from relay" "$SCRATCH/t13_sc.log")
if [[ "$WELCOMES" -ge 2 ]]; then
  pass "T13c re-joined after server returned ($WELCOMES welcomes seen)"
else
  fail "T13c reconnect" "welcome count=$WELCOMES"
fi

kill $SV2 $SC 2>/dev/null
wait 2>/dev/null

# ============================================================================
hdr "T14: Server auto-registers with master (HSMP_MASTER_URL)"

e2e_port MPORT2
e2e_port PORT3
"$BINS/hsmp-master.exe" --parent-pid "$E2E_WINPID" --bind "127.0.0.1:$MPORT2" > "$SCRATCH/t14_mp.log" 2>&1 &
MP2=$!
e2e_wait_bound $MPORT2 $MP2
HSMP_MASTER_URL="http://127.0.0.1:$MPORT2" "$BINS/hsmp-server.exe" \
  --bind "127.0.0.1:$PORT3" --tick-hz 30 --name "Registered Server" --mode "duel" \
  > "$SCRATCH/t14_sv.log" 2>&1 &
SV3=$!
sleep 2

LIST_REG=$(curl -s "http://127.0.0.1:$MPORT2/v1/servers")
if echo "$LIST_REG" | grep -q '"name":"Registered Server"'; then
  pass "T14 server auto-registered with master"
else
  fail "T14 auto-register" "list: $LIST_REG"
fi

kill $SV3 $MP2 2>/dev/null
wait 2>/dev/null

# ============================================================================
hdr "V1-V5: Build identity (protocol / content checks, listing fields)"

V_HASH=$("$BINS/hsmp-server.exe" --build-info | grep -o '"content_hash":"[0-9a-f]*"' | cut -d'"' -f4)
V_VER=$("$BINS/hsmp-server.exe" --build-info | grep -o '"version":"[^"]*"' | cut -d'"' -f4)
V_OTHER=$(printf '%064d' 7)
e2e_port VMPORT
e2e_port VPORT
e2e_port VPORT2
"$BINS/hsmp-master.exe" --parent-pid "$E2E_WINPID" --bind "127.0.0.1:$VMPORT" > "$SCRATCH/v_mp.log" 2>&1 &
VMP=$!
e2e_wait_bound $VMPORT $VMP
# the default server: content check on, registered with the master
HSMP_MASTER_URL="http://127.0.0.1:$VMPORT" "$BINS/hsmp-server.exe" --parent-pid "$E2E_WINPID" \
  --bind "127.0.0.1:$VPORT" --tick-hz 30 --name "Versioned Server" > "$SCRATCH/v_sv.log" 2>&1 &
VSV=$!
# a development server: --allow-mismatched-content
"$BINS/hsmp-server.exe" --parent-pid "$E2E_WINPID" --bind "127.0.0.1:$VPORT2" --tick-hz 30 \
  --allow-mismatched-content > "$SCRATCH/v_sv2.log" 2>&1 &
VSV2=$!
e2e_wait_bound $VPORT $VSV && e2e_wait_bound $VPORT2 $VSV2
DIRV_P="$SCRATCH/sideVp"; DIRV_C="$SCRATCH/sideVc"; DIRV_A="$SCRATCH/sideVa"; DIRV_OK="$SCRATCH/sideVok"
mkdir -p "${DIRV_P:?}" "${DIRV_C:?}" "${DIRV_A:?}" "${DIRV_OK:?}"

# V1: an older protocol (v5) is refused with VERSION (1) and both versions named, not a hang
sc_launch "$DIRV_P" --parent-pid "$E2E_WINPID" --server "127.0.0.1:$VPORT" --state-dir "$DIRV_P" --nick "OldProto" \
  --proto-min 5 --proto-max 5 > "$SCRATCH/v_scP.log" 2>&1 &
VS1=$!
wait_link 5 "$DIRV_P" rejected
if link_is "$DIRV_P" rejected && [[ "$(link_get "$DIRV_P" reason_code)" == "1" ]] \
   && link_get "$DIRV_P" reason | grep -q "Server runs HalfSword-MP $V_VER, you have $V_VER" \
   && link_get "$DIRV_P" reason | grep -q "OUTDATED: update via the launcher"; then
  pass "V1 protocol mismatch refused: code 1, \"$(link_get "$DIRV_P" reason)\""
else
  fail "V1 protocol mismatch" "$(link_show "$DIRV_P") code=$(link_get "$DIRV_P" reason_code)"
fi

# V2: other mod files are refused by default with CONTENT (2)
sc_launch "$DIRV_C" --parent-pid "$E2E_WINPID" --server "127.0.0.1:$VPORT" --state-dir "$DIRV_C" --nick "OtherMods" \
  --content-hash "$V_OTHER" > "$SCRATCH/v_scC.log" 2>&1 &
VS2=$!
wait_link 5 "$DIRV_C" rejected
if link_is "$DIRV_C" rejected && [[ "$(link_get "$DIRV_C" reason_code)" == "2" ]] \
   && link_get "$DIRV_C" reason | grep -q "mod files differ: repair via the launcher" \
   && nocolor "$SCRATCH/v_sv.log" | grep -q "content check on"; then
  pass "V2 content mismatch refused when enforced: code 2, \"$(link_get "$DIRV_C" reason)\""
else
  fail "V2 content mismatch" "$(link_show "$DIRV_C") code=$(link_get "$DIRV_C" reason_code)"
fi

# V3: the same build joins the enforcing server
sc_launch "$DIRV_OK" --parent-pid "$E2E_WINPID" --server "127.0.0.1:$VPORT" --state-dir "$DIRV_OK" --nick "SameBuild" \
  > "$SCRATCH/v_scOK.log" 2>&1 &
VS3=$!
wait_link 5 "$DIRV_OK" connected
if link_is "$DIRV_OK" connected; then pass "V3 same build joins the enforcing server"
else fail "V3 same build" "$(link_show "$DIRV_OK")"; fi

# V4: the escape hatch lets other mod files in
sc_launch "$DIRV_A" --parent-pid "$E2E_WINPID" --server "127.0.0.1:$VPORT2" --state-dir "$DIRV_A" --nick "DevMods" \
  --content-hash "$V_OTHER" > "$SCRATCH/v_scA.log" 2>&1 &
VS4=$!
wait_link 5 "$DIRV_A" connected
if link_is "$DIRV_A" connected && nocolor "$SCRATCH/v_sv2.log" | grep -q "content check OFF"; then
  pass "V4 --allow-mismatched-content admits other mod files"
else
  fail "V4 escape hatch" "$(link_show "$DIRV_A")"
fi

# V5: the listing carries version, protocol range and the enforced content hash; hsmp-query passes them on
V_LIST=$(curl -s "http://127.0.0.1:$VMPORT/v1/servers")
V_Q=$("$BINS/hsmp-query.exe" --master "http://127.0.0.1:$VMPORT" --timeout-ms 800 2>/dev/null | grep "^S" | head -1)
if echo "$V_LIST" | grep -q "\"content_hash\":\"$V_HASH\"" && echo "$V_LIST" | grep -q '"proto_min":6' \
   && echo "$V_LIST" | grep -q '"proto_max":6' && echo "$V_LIST" | grep -q "\"version\":\"$V_VER\"" \
   && [[ "$(echo "$V_Q" | cut -f16-18)" == "${V_HASH:0:16}"$'\t'"6"$'\t'"6" ]]; then
  pass "V5 listing: version $V_VER, proto 6..6, content_hash ${V_HASH:0:16}..."
else
  fail "V5 listing fields" "list: $V_LIST | query: $V_Q"
fi
kill $VS1 $VS2 $VS3 $VS4 $VSV $VSV2 $VMP 2>/dev/null
wait 2>/dev/null

# ============================================================================
hdr "T16: Security negatives — wrong-proto, server-full"

e2e_port PORT4
"$BINS/hsmp-server.exe" --parent-pid "$E2E_WINPID" --bind "127.0.0.1:$PORT4" --tick-hz 30 --max-peers 1 > "$SCRATCH/t16_sv.log" 2>&1 &
SV4=$!
e2e_wait_bound $PORT4 $SV4
DIR_ok="$SCRATCH/t16_ok"
DIR_full="$SCRATCH/t16_full"
mkdir -p "${DIR_ok:?}" "${DIR_full:?}"

sc_launch "$DIR_ok" --parent-pid "$E2E_WINPID" --server "127.0.0.1:$PORT4" --state-dir "$DIR_ok" --nick "First" > "$SCRATCH/t16_scOk.log" 2>&1 &
SC_OK=$!
sleep 0.8
sc_launch "$DIR_full" --parent-pid "$E2E_WINPID" --server "127.0.0.1:$PORT4" --state-dir "$DIR_full" --nick "Rejected" > "$SCRATCH/t16_scFull.log" 2>&1 &
SC_FULL=$!
sleep 1.5

# "Rejected" should have gotten S2CReject with "server full"
if grep -q "rejected us" "$SCRATCH/t16_scFull.log"; then
  pass "T16a server-full rejection delivered"
else
  fail "T16a server-full" "scFull log: $(tail -5 $SCRATCH/t16_scFull.log)"
fi

kill $SV4 $SC_OK $SC_FULL 2>/dev/null
wait 2>/dev/null

# ============================================================================
hdr "T18-T24: Match loop, admin/kick/ban"

e2e_port PORT5
DIRX="$SCRATCH/sideX"; DIRY="$SCRATCH/sideY"
mkdir -p "${DIRX:?}" "${DIRY:?}"

# Security: X is the listen host (admin by its player key file).
"$BINS/hsmp-server.exe" --parent-pid "$E2E_WINPID" --bind "127.0.0.1:$PORT5" --tick-hz 30 --max-peers 4 --owner-key-file "$DIRX/.player_key" > "$SCRATCH/t18_sv.log" 2>&1 &
SV5=$!
e2e_wait_bound $PORT5 $SV5
sc_launch "$DIRX" --parent-pid "$E2E_WINPID" --server "127.0.0.1:$PORT5" --state-dir "$DIRX" --nick "AdminGuy" > "$SCRATCH/t18_scX.log" 2>&1 &
SX=$!
# X joins first (peer 1; it is admin by its key, not by joining first): wait for its handshake
wait_link 10 "$DIRX" connected || sleep 0.6
sc_launch "$DIRY" --parent-pid "$E2E_WINPID" --server "127.0.0.1:$PORT5" --state-dir "$DIRY" --nick "Victim" > "$SCRATCH/t18_scY.log" 2>&1 &
SY=$!
wait_link 10 "$DIRY" connected
wait_admin 3 "$DIRX" true

# T18a: X is admin (the listen host's key): the link record's admin flag
if link_admin_is "$DIRX" true; then
  pass "T18a listen host (owner key) gets is_admin=true"
else fail "T18a admin" "$(link_show "$DIRX")"; fi
if link_admin_is "$DIRY" false; then
  pass "T18b second peer is not admin"
else fail "T18b admin" "$(link_show "$DIRY")"; fi

# T19 (character build) and T20 (legacy C2SHit hit event) are gone with their protocol
# bodies: character builds had no reader, hits are combat's damage path.

# T21: match loop — Y dies → X wins the round. Ready both, start, wait for live.
send_ready "$DIRX" 1801
send_ready "$DIRY" 2801
wait_cmd_result 3 "$DIRX" 1801; wait_cmd_result 3 "$DIRY" 2801
send_cmd "$DIRX" 1802 start
if wait_phase 8 "$DIRX" live; then
  pass "T21a match in live state"
else fail "T21a live" "$(sess_show "$DIRX") | $(cmd_result "$DIRX" 1802)"; fi

# T20: a damage claim in the live round. Keep the attacker's position near the target via a
# root (me_*.json) so the server's range check has it.
echo '{"pid":9100,"nick":"AdminGuy","tick":1,"pos":[-480.0,0.0,100.0],"rot":[0.0,0.0,0.0],"vel":[0.0,0.0,0.0]}' > "$DIRX/me_9100.json"
# (No root from Y: a victim without stream history is judged without lag compensation,
# so this un-timestamped claim is accepted; lagcomp itself is covered by unit tests.)
sleep 0.5
# The game's claim (a typed `damage` record, cid 7): the sidecar fills hit_id / round /
# age_ms; the server validates it, forwards it to Y as `damage_in` (peer = X), confirms to X,
# and on Y's sidecar ack sends X the FINAL verdict, all as records into the games' S2G rings.
rec_put "$DIRX" damage '{"cid":7,"target_peer_id":2,"bone":"spine_02","raw_damage":10.0,"damage_out":5.0,"cutting_power":1.0,"normal":[1.0,0.0,0.0],"location":[-500.0,0.0,110.0],"offset":[0.0,0.0,10.0],"flags":32}'
if e2e_wait_file 5 "$DIRY/view.jsonl" '"ev":"s2g".*"kind":"damage_in"'; then
  DI=$(grep '"ev":"s2g".*"kind":"damage_in"' "$DIRY/view.jsonl" | tail -1)
  if echo "$DI" | grep -q '"bone":"spine_02"' && echo "$DI" | grep -q '"cid":7' && echo "$DI" | grep -q '"target_peer_id":2'; then
    pass "T20 damage claim reaches the victim's game as damage_in (record as written)"
  else fail "T20 damage_in" "$(echo "$DI" | head -c 400)"; fi
else
  fail "T20 damage claim" "no damage_in in Y's view; X: $(grep '"kind":"damage_verdict"' "$DIRX/view.jsonl" | tail -1 | head -c 300)"
fi
if e2e_wait_file 5 "$DIRX/view.jsonl" '"kind":"damage_verdict".*"kind":2.*"ok":true'; then
  if grep '"kind":"damage_verdict"' "$DIRX/view.jsonl" | grep -q '"cid":7'; then
    pass "T20a attacker gets the FINAL verdict for its claim id (owner acked)"
  else fail "T20a verdict cid" "$(grep '"kind":"damage_verdict"' "$DIRX/view.jsonl" | tail -1 | head -c 300)"; fi
else
  fail "T20a damage verdict" "$(grep '"kind":"damage_verdict"' "$DIRX/view.jsonl" | tail -2 | head -c 400)"
fi

# Y's game reports its own death (typed death_report, round 0 = the sidecar's round):
# resent until death_ack; the server declares it and every game gets one `death` record.
rec_put "$DIRY" death_report '{"round":0}'
wait_phase 4 "$DIRX" roundover
T21_RO=no; phase_is "$DIRX" roundover && T21_RO=yes
# The roster row's wins can land a snapshot after the phase: let it converge (3 s).
t21_wins() { jget "$(row_of_peer "$DIRX" 1)" wins | grep -qx 1; }
rec_wait 3 t21_wins
if [[ "$T21_RO" == yes ]] && t21_wins; then
  pass "T21b player died → roundover, admin wins=1"
else fail "T21b roundover" "$(sess_show "$DIRX" 600)"; fi
if e2e_wait_file 3 "$DIRX/view.jsonl" '"ev":"s2g".*"kind":"death".*"peer_id":2' \
   && grep -q '"ev":"s2g".*"kind":"death".*"peer_id":2' "$DIRY/view.jsonl"; then
  pass "T21c death record reaches both games (S2G, match_id filled)"
else fail "T21c death record" "$(grep '"kind":"death"' "$DIRX/view.jsonl" | tail -1 | head -c 300)"; fi

# T22: admin kick — X (admin) kicks peer_id=2 (Y): a typed KICK command
send_cmd "$DIRX" 1803 kick '"peer_id":2,"text":"kicked"'
wait_link 4 "$DIRY" kicked
if link_is "$DIRY" kicked; then
  pass "T22a kicked peer: link status kicked (terminal)"
else
  fail "T22a kick" "$(link_show "$DIRY")"
fi
# And X's game sees the kick announced in chat (a chat_in record from the server)
if wait_chat 3 "$DIRX" "kicked by AdminGuy"; then
  pass "T22b kick broadcast seen in chat"
else
  fail "T22b kick chat" "$(s2g "$DIRX" chat_in | tail -3 | head -c 400)"
fi

# T23: the admin state reached the admin's game (slot `admin`; a kick is not a ban: no rows)
if [[ -n "$(rec "$DIRX" admin)" && "$(jget "$(rec_v "$DIRX" admin)" admin_peer)" == 1 ]]; then
  pass "T23 admin record: the admin sees itself as admin (kick ≠ ban: no ban rows)"
else
  fail "T23 admin record" "$(rec "$DIRX" admin | head -c 300)"
fi

# T24: non-admin can't admin — new sidecar in fresh state-dir
DIRZ="$SCRATCH/sideZ"
mkdir -p "${DIRZ:?}"
sc_launch "$DIRZ" --parent-pid "$E2E_WINPID" --server "127.0.0.1:$PORT5" --state-dir "$DIRZ" --nick "NonAdmin" > "$SCRATCH/t24_scZ.log" 2>&1 &
SZ=$!
wait_link 10 "$DIRZ" connected
# Verify Z is up and NOT admin
if link_is "$DIRZ" connected && link_admin_is "$DIRZ" false; then
  pass "T24a NonAdmin joined (is_admin=false)"
else
  fail "T24a NonAdmin join" "$(link_show "$DIRZ")"
fi
send_cmd "$DIRZ" 2401 kick '"peer_id":1'
wait_cmd_result 3 "$DIRZ" 2401
if cmd_ok "$DIRZ" 2401 false && [[ "$(cmd_reason "$DIRZ" 2401)" == NOT_ADMIN ]]; then
  pass "T24 non-admin kick refused (NOT_ADMIN)"
else
  fail "T24 non-admin" "$(cmd_result "$DIRZ" 2401)"
fi

kill $SV5 $SX $SY $SZ 2>/dev/null
wait 2>/dev/null

# ============================================================================
hdr "T25-T30: NAT rendezvous, RTT, respawn, history"

e2e_port PORT6
e2e_port MPORT6
e2e_port RDV_LOCAL_PORT
DIRP="$SCRATCH/sidePing"
DIRV1="$SCRATCH/sideV1"
DIRV2="$SCRATCH/sideV2"
mkdir -p "${DIRP:?}" "${DIRV1:?}" "${DIRV2:?}"
for d in "$DIRP" "$DIRV1" "$DIRV2"; do
  rm -f "${d:?}"/.history.jsonl "${d:?}"/.control.outbox.jsonl 2>/dev/null || true
done
# the server writes history.jsonl to its working directory: a per-run one (never the repo)
T26_WD="$SCRATCH/t26_wd"; mkdir -p "$T26_WD"

# T25: NAT rendezvous
"$BINS/hsmp-master.exe" --parent-pid "$E2E_WINPID" --bind "127.0.0.1:$MPORT6" > "$SCRATCH/t25_mp.log" 2>&1 &
MP6=$!
e2e_wait_bound $MPORT6 $MP6
MYADDR=$(curl -s "http://127.0.0.1:$MPORT6/v1/myaddr")
if echo "$MYADDR" | grep -q '"ip":"127.0.0.1"'; then
  pass "T25a /v1/myaddr returns observed IP"
else
  fail "T25a myaddr" "$MYADDR"
fi
RD1=$(curl -s -X POST "http://127.0.0.1:$MPORT6/v1/rendezvous/testroom" -d '{}' -H "content-type: application/json")
if echo "$RD1" | grep -q '"peers":\[\]'; then
  pass "T25b first rendezvous: empty peers"
else fail "T25b rendezvous first" "$RD1"; fi
RD2=$(curl -s -X POST "http://127.0.0.1:$MPORT6/v1/rendezvous/testroom" -d '{}' -H "content-type: application/json" --local-port $RDV_LOCAL_PORT)
if echo "$RD2" | grep -q '"peers":\["127.0.0.1'; then
  pass "T25c second rendezvous: sees first peer"
else fail "T25c rendezvous second" "$RD2"; fi

kill $MP6 2>/dev/null; wait 2>/dev/null

# T26-T30: live stack for RTT, respawn (spawn plan), history
pushd "$T26_WD" >/dev/null
"$BINS/hsmp-server.exe" --parent-pid "$E2E_WINPID" --bind "127.0.0.1:$PORT6" --tick-hz 30 --max-peers 4 --owner-key-file "$DIRV1/.player_key" > "$SCRATCH/t26_sv.log" 2>&1 &
SV6=$!
popd >/dev/null
e2e_wait_bound $PORT6 $SV6
sc_launch "$DIRV1" --parent-pid "$E2E_WINPID" --server "127.0.0.1:$PORT6" --state-dir "$DIRV1" --nick "V1" > "$SCRATCH/t26_scV1.log" 2>&1 &
SV1_PID=$!
wait_link 10 "$DIRV1" connected || sleep 0.6
sc_launch "$DIRV2" --parent-pid "$E2E_WINPID" --server "127.0.0.1:$PORT6" --state-dir "$DIRV2" --nick "V2" > "$SCRATCH/t26_scV2.log" 2>&1 &
SV2_PID=$!
sleep 2.5  # RTT ping fires every 2s

# T26: RTT metrics in the link record (a ping / pong sample: metrics_wall_ms set, rtt_ms)
T26_L=$(rec_v "$DIRV1" link)
if [[ "$(jget "$T26_L" metrics_wall_ms)" =~ ^[1-9][0-9]*$ && -n "$(jget "$T26_L" rtt_ms)" ]]; then
  pass "T26 RTT metrics in the link record (rtt_ms $(jget "$T26_L" rtt_ms), srtt_ms $(jget "$T26_L" srtt_ms))"
else
  fail "T26 RTT" "${T26_L:0:400}"
fi

# T27 (arena list) is gone with its protocol body: the arena catalogue is the
# session config.

# T28: a damage claim outside a live round is rejected (anti-cheat): the game gets a FINAL
# verdict ok=false for its claim id (a damage_verdict record; nothing reaches the target).
rec_put "$DIRV1" damage '{"cid":28,"target_peer_id":2,"bone":"head","damage_out":5.0,"normal":[1.0,0.0,0.0]}'
if e2e_wait_file 5 "$DIRV1/view.jsonl" '"kind":"damage_verdict".*"cid":28.*"kind":2.*"ok":false'; then
  pass "T28 damage rejected when not live (FINAL verdict, claim id 28)"
else
  fail "T28 damage reject" "$(grep '"kind":"damage_verdict"' "$DIRV1/view.jsonl" 2>/dev/null | tail -1 | head -c 300)"
fi

# T29: admin sets best_of=1 → single-round match → match_over + history write.
send_cmd "$DIRV1" 2901 set_config "\"patch\":{\"mask\":$REC_CFG_BEST_OF,\"best_of\":1}"
wait_cmd_result 3 "$DIRV1" 2901
send_ready "$DIRV1" 2902
send_ready "$DIRV2" 2903
wait_cmd_result 3 "$DIRV1" 2902; wait_cmd_result 3 "$DIRV2" 2903
send_cmd "$DIRV1" 2904 start
# Poll for live state up to 6 s.
LIVE=""
wait_phase 6 "$DIRV1" live && LIVE=1
# Spawns are a server plan carried in every session snapshot (the roster's spawn orders):
# the live round still has one order per player for round 1.
T29_N=0
for sid in $(sess_rows "$DIRV1" | grep -o '"spawn_id":[0-9]*' | cut -d: -f2); do (( (sid >> 8) == 1 )) && T29_N=$((T29_N+1)); done
if [[ "$LIVE" == "1" && "$T29_N" -eq 2 ]] && cmd_ok "$DIRV1" 2901 true; then
  pass "T29a best_of=1 accepted; server spawn plan present in the live round"
else
  fail "T29a spawn plan" "$(cmd_result "$DIRV1" 2901) | $(sess_show "$DIRV1" 600)"
fi

rec_put "$DIRV2" death_report '{"round":0}'
sleep 8  # 5s match_over pause + 1s die + buffer → reset to lobby + history write

# T30: history.jsonl written on match_over
# The server writes "history.jsonl" to its current working directory: the
# per-run $T26_WD the test launched it in.
HISTORY_LOCATIONS=("$T26_WD/history.jsonl")
FOUND_HIST=""
for h in "${HISTORY_LOCATIONS[@]}"; do
  if [[ -f "$h" ]] && grep -q '"winner_id"' "$h" 2>/dev/null; then
    FOUND_HIST="$h"; break
  fi
done
if [[ -n "$FOUND_HIST" ]]; then
  pass "T30 server wrote history.jsonl ($FOUND_HIST)"
else
  fail "T30 history" "no history.jsonl found with winner_id entries"
fi
# The client-side .history.jsonl has no reader and is not written (T30 checks the server's).
if [[ ! -e "$DIRV1/.history.jsonl" ]]; then
  pass "T30b dead client .history contract not written (A0)"
else
  fail "T30b client history" "dead contract .history.jsonl written"
fi

# T31 (voice) is gone with its protocol bodies (no game-side writer or reader).

kill $SV6 $SV1_PID $SV2_PID 2>/dev/null
wait 2>/dev/null

# ============================================================================
hdr "T32-T35: Dedicated-server RCON + persistent banlist"

e2e_port PORT_RCON
e2e_port RCON_PORT
BANS_FILE="$SCRATCH/bans.txt"
rm -f "${BANS_FILE:?}" 2>/dev/null || true

"$BINS/hsmp-server.exe" --parent-pid "$E2E_WINPID" --bind "127.0.0.1:$PORT_RCON" --tick-hz 30 \
  --bans-file "$BANS_FILE" \
  --rcon-bind "127.0.0.1:$RCON_PORT" --rcon-password e2epw \
  > "$SCRATCH/t32_sv.log" 2>&1 &
SV_RCON=$!
e2e_wait_bound $PORT_RCON $SV_RCON && e2e_wait_bound $RCON_PORT $SV_RCON

DIR_RC="$SCRATCH/sideRc"
mkdir -p "${DIR_RC:?}"
sc_launch "$DIR_RC" --parent-pid "$E2E_WINPID" --server "127.0.0.1:$PORT_RCON" --state-dir "$DIR_RC" --nick "RconVictim" > "$SCRATCH/t32_sc.log" 2>&1 &
SC_RC=$!
sleep 1

# T32: RCON auth
AUTH_RESP=$("$HSMP_TOOLS" rcon "127.0.0.1:$RCON_PORT" "AUTH e2epw@300")
if [[ "$AUTH_RESP" == "OK authenticated" ]]; then
  pass "T32 RCON authentication"
else
  fail "T32 RCON auth" "got: $AUTH_RESP"
fi

# T33: RCON bad password rejected
BAD_RESP=$("$HSMP_TOOLS" rcon "127.0.0.1:$RCON_PORT" "AUTH wrong@300")
if [[ "$BAD_RESP" == "ERR bad password" ]]; then
  pass "T33 RCON wrong-password rejected"
else
  fail "T33 RCON bad pw" "$BAD_RESP"
fi

# T34: RCON BAN + persistent file write
BAN_RESP=$("$HSMP_TOOLS" rcon "127.0.0.1:$RCON_PORT" "AUTH e2epw@200" "BAN 1@500")
sleep 0.5
if echo "$BAN_RESP" | grep -q "OK banned 127.0.0.1"; then
  pass "T34a RCON BAN replied with IP"
else
  fail "T34a BAN" "$BAN_RESP"
fi
if [[ -f "$BANS_FILE" ]] && grep -q "127.0.0.1" "$BANS_FILE"; then
  pass "T34b bans.txt persisted to disk"
else
  fail "T34b bans.txt" "$(cat $BANS_FILE 2>/dev/null)"
fi

kill $SV_RCON $SC_RC 2>/dev/null; wait 2>/dev/null

# T35: banlist loaded on restart; banned IP's rejoin is refused
"$BINS/hsmp-server.exe" --parent-pid "$E2E_WINPID" --bind "127.0.0.1:$PORT_RCON" --tick-hz 30 \
  --bans-file "$BANS_FILE" > "$SCRATCH/t35_sv.log" 2>&1 &
SV_RC2=$!
e2e_wait_bound $PORT_RCON $SV_RC2
sc_launch "$DIR_RC" --parent-pid "$E2E_WINPID" --server "127.0.0.1:$PORT_RCON" --state-dir "$DIR_RC" --nick "BannedTry" > "$SCRATCH/t35_sc.log" 2>&1 &
SC_RC2=$!
sleep 1.5
if grep -q "banned IP tried to join\|rejected us.*banned" "$SCRATCH/t35_sv.log" "$SCRATCH/t35_sc.log" 2>/dev/null; then
  pass "T35 banlist loaded on startup — banned IP refused"
else
  fail "T35 banlist persist" "no 'banned' in server/sidecar logs"
fi

kill $SV_RC2 $SC_RC2 2>/dev/null; wait 2>/dev/null

# ============================================================================
hdr "T36-T48: session — S2CSession, typed commands, RCON match/debug verbs, events, pid files, parent-pid"

e2e_port PORT_S
e2e_port RCON_S
DIRS1="$SCRATCH/sess1"; DIRS2="$SCRATCH/sess2"; DIRSP="$SCRATCH/sess_pids"
mkdir -p "${DIRS1:?}" "${DIRS2:?}" "${DIRSP:?}"
SV_EVENTS="$SCRATCH/sess_server.jsonl"
rcon_s() { "$HSMP_TOOLS" rcon "127.0.0.1:$RCON_S" "AUTH sesspw@150" "$@"; }
# Fake "game" processes for --parent-pid (Windows pids of two MSYS sleeps).
sleep 600 & FAKE1=$!
sleep 600 & FAKE2=$!
WFAKE1=$(cat /proc/$FAKE1/winpid 2>/dev/null || echo $FAKE1)
WFAKE2=$(cat /proc/$FAKE2/winpid 2>/dev/null || echo $FAKE2)

"$BINS/hsmp-server.exe" --parent-pid "$E2E_WINPID" --bind "127.0.0.1:$PORT_S" --tick-hz 30 --max-peers 4 \
  --rcon-bind "127.0.0.1:$RCON_S" --rcon-password sesspw --debug-verbs --owner-key-file "$DIRS1/.player_key" \
  --events "$SV_EVENTS" --pid-file "$DIRSP/.pid.server.json" \
  > "$SCRATCH/t36_sv.log" 2>&1 &
SVS=$!
e2e_wait_bound $PORT_S $SVS && e2e_wait_bound $RCON_S $SVS
# Both players are called "Willie": the server deduplicates the second one.
sc_launch "$DIRS1" --server "127.0.0.1:$PORT_S" --state-dir "$DIRS1" --nick "Willie" \
  --events "$DIRS1/sidecar.jsonl" --parent-pid "$WFAKE1" > "$SCRATCH/t36_s1.log" 2>&1 &
SS1=$!
wait_link 10 "$DIRS1" connected || sleep 0.6
sc_launch "$DIRS2" --server "127.0.0.1:$PORT_S" --state-dir "$DIRS2" --nick "Willie" \
  --events "$DIRS2/sidecar.jsonl" --parent-pid "$WFAKE2" > "$SCRATCH/t36_s2.log" 2>&1 &
SS2=$!
wait_link 10 "$DIRS2" connected
two_rows() { [[ "$(sess_nrows "$1")" -eq 2 ]]; }
rec_wait 5 two_rows "$DIRS1"; rec_wait 5 two_rows "$DIRS2"

# T36: the session snapshot record on both clients (same epoch, roster by seat)
EP1=$(sess_get "$DIRS1" epoch); EP2=$(sess_get "$DIRS2" epoch)
if phase_is "$DIRS1" lobby && [[ "$(sess_nrows "$DIRS1")" -eq 2 ]] \
   && [[ "$(my_seat "$DIRS1")" == 1 && "$(my_seat "$DIRS2")" == 2 ]] \
   && [[ -n "$EP1" && "$EP1" == "$EP2" ]] && [[ "$(sess_get "$DIRS1" has_frozen)" == false ]]; then
  pass "T36 session record: lobby snapshot, 2 seats, my_seat 1/2, same epoch, no frozen config"
else
  fail "T36 session snapshot" "S1: seat $(my_seat "$DIRS1") $(sess_show "$DIRS1" 400) | S2: seat $(my_seat "$DIRS2") epoch $EP2"
fi

# T37: nick dedup
if sess_rows "$DIRS1" | grep -q '"nick":"Willie"' && sess_rows "$DIRS1" | grep -q '"nick":"Willie (2)"'; then
  pass "T37 duplicate nick deduplicated (Willie, Willie (2))"
else
  fail "T37 nick dedup" "$(sess_rows "$DIRS1" | grep -o '"nick":"[^"]*"' | tr '\n' ' ')"
fi

# T38: typed commands with results (G2S `command` record -> server -> S2G `cmd_result`)
send_cmd "$DIRS1" 101 start
send_pick "$DIRS2" 201 Pit
send_pick "$DIRS1" 102 Pit
send_pick "$DIRS1" 103 Nowhere
for id in 101 102 103; do wait_cmd_result 3 "$DIRS1" $id; done
wait_cmd_result 3 "$DIRS2" 201
if cmd_ok "$DIRS1" 101 false && [[ "$(cmd_reason "$DIRS1" 101)" == NOT_ALL_READY ]] \
   && jget "$(cmd_result "$DIRS1" 101)" reason_text | grep -q 'start blocked: 0 of 2 peers ready'; then
  pass "T38a START refused with a reason (NOT_ALL_READY, 0 of 2 ready)"
else fail "T38a start refused" "$(cmd_result "$DIRS1" 101)"; fi
if [[ "$(cmd_reason "$DIRS2" 201)" == NOT_ADMIN ]]; then
  pass "T38b non-admin pick_arena refused (NOT_ADMIN)"
else fail "T38b not admin" "$(cmd_result "$DIRS2" 201)"; fi
pit_arena() { [[ "$(sess_arena "$1")" == Map_Arena_Pit ]]; }
rec_wait 3 pit_arena "$DIRS1"
if cmd_ok "$DIRS1" 102 true && pit_arena "$DIRS1"; then
  pass "T38c pick_arena accepted; snapshot config.arena = Map_Arena_Pit"
else fail "T38c pick" "$(cmd_result "$DIRS1" 102) | $(sess_show "$DIRS1")"; fi
if [[ "$(cmd_reason "$DIRS1" 103)" == UNKNOWN_ARENA ]]; then
  pass "T38d unknown arena refused (UNKNOWN_ARENA)"
else fail "T38d unknown arena" "$(cmd_result "$DIRS1" 103)"; fi

# T39: a resent command id gets no second result (exactly one per cmd_id)
send_cmd "$DIRS1" 101 start
sleep 0.8
N101=$(cmd_results_n "$DIRS1" 101)
if [[ "$N101" -eq 1 ]]; then pass "T39 exactly one result for a resent cmd_id"
else fail "T39 result dedup" "cmd_id 101 results: $N101"; fi

# T40: READY commands from both games -> results ok; both ready in the snapshot
send_ready "$DIRS1" 104
send_ready "$DIRS2" 202
wait_cmd_result 3 "$DIRS1" 104; wait_cmd_result 3 "$DIRS2" 202
both_ready() { [[ "$(sess_rows "$1" | grep -c '"ready":true')" -eq 2 ]]; }
rec_wait 3 both_ready "$DIRS1"
if cmd_ok "$DIRS1" 104 true && cmd_ok "$DIRS2" 202 true && both_ready "$DIRS1"; then
  pass "T40 READY {cmd_id} -> result ok; both ready in the snapshot"
else fail "T40 ready" "$(cmd_result "$DIRS1" 104) | $(cmd_result "$DIRS2" 202) | $(sess_show "$DIRS1" 400)"; fi

# T41: RCON match verbs through the same command path
ST0=$(rcon_s "STATUS@400")
MAPR=$(rcon_s "MAP Yard@400")
BOR=$(rcon_s "BESTOF 19@400")
if echo "$ST0" | grep -q '"phase":"lobby"' && echo "$MAPR" | grep -q "OK Map_Arena_Yard" && echo "$BOR" | grep -q "^OK"; then
  pass "T41a RCON STATUS / MAP Yard / BESTOF 19"
else fail "T41a rcon verbs" "$ST0 | $MAPR | $BOR"; fi
send_ready "$DIRS2" 203 false
wait_cmd_result 3 "$DIRS2" 203
STR=$(rcon_s "START@400")
if echo "$STR" | grep -q "ERR start blocked: 1 of 2 peers ready" && phase_is "$DIRS1" lobby; then
  pass "T41b RCON START with an unready player -> ERR, still lobby"
else fail "T41b start refused" "$STR"; fi
send_ready "$DIRS2" 204 true
wait_cmd_result 3 "$DIRS2" 204
STR=$(rcon_s "START@400")
frozen_yard() { [[ "$(sess_get "$1" has_frozen)" == true && "$(sess_arena "$1")" == Map_Arena_Yard ]]; }
rec_wait 3 frozen_yard "$DIRS1"
if echo "$STR" | grep -q "OK starting on Map_Arena_Yard" && frozen_yard "$DIRS1" \
   && [[ "$(sess_get "$DIRS1" match_id)" =~ ^[1-9][0-9]*$ ]]; then
  pass "T41c RCON START -> frozen config (Yard), match_id set"
else fail "T41c start" "$STR | $(sess_show "$DIRS1" 500)"; fi
LIVE_S=""
wait_phase 4 "$DIRS1" live && LIVE_S=1
SP_IDS=$(sess_rows "$DIRS1" | grep -o '"spawn_id":[1-9][0-9]*' | sort -u | wc -l)
if [[ "$LIVE_S" == "1" && "$SP_IDS" -eq 2 ]]; then
  pass "T41d live round 1 with 2 distinct spawn orders in the roster"
else fail "T41d live" "$(sess_show "$DIRS1" 600)"; fi

# T42: DEBUG KILL <seat> ends the round (gate round driver)
KR=$(rcon_s "DEBUG KILL 2@400")
wait_phase 3 "$DIRS1" roundover
if echo "$KR" | grep -q "OK killed seat 2" && phase_is "$DIRS1" roundover; then
  pass "T42 RCON DEBUG KILL 2 -> roundover"
else fail "T42 debug kill" "$KR | $(sess_show "$DIRS1")"; fi
seat1_won() { [[ "$(jget "$(row_of_seat "$1" 1)" wins)" == 1 && "$(sess_get "$1" winner_seat)" == 1 ]]; }
rec_wait 3 seat1_won "$DIRS1"
if seat1_won "$DIRS1"; then
  pass "T42b seat 1 won the round (wins=1, winner_seat=1)"
else fail "T42b round result" "$(sess_show "$DIRS1" 600)"; fi

# T43: ABORT -> lobby, frozen cleared, last result ABORTED (result_reason 6)
AB=$(rcon_s "ABORT@400")
wait_phase 3 "$DIRS1" lobby
if echo "$AB" | grep -q "OK aborted" && phase_is "$DIRS1" lobby \
   && [[ "$(sess_get "$DIRS1" has_frozen)" == false && "$(sess_get "$DIRS1" result_reason)" == 6 ]]; then
  pass "T43 RCON ABORT -> lobby (no frozen config, last_result aborted)"
else fail "T43 abort" "$AB | $(sess_show "$DIRS1" 400)"; fi

# T49: a client's load error never stalls the barrier (one 10 s retry window),
# and a duel with one fighter loaded is void instead of stuck or "won".
# Both "games" report a game status first (they take part in the load barrier from now on).
send_status "$DIRS1" 0 0
send_status "$DIRS2" 0 0
sleep 0.6
FS=$(rcon_s "START FORCE@400")
wait_phase 3 "$DIRS1" loading
send_status "$DIRS1" 1 0 Map_Arena_Yard
send_status "$DIRS2" 0 0 "" spawn_timeout
sleep 5
MID_PHASE=$(sess_phase "$DIRS1"); MID_W2=$(jget "$(row_of_seat "$DIRS1" 2)" waiting)
sleep 8
T49_N=$(s2g "$DIRS1" notice | grep '"code":3,' | grep -c '"spawn_timeout"')
if echo "$FS" | grep -q "OK starting" && [[ "$MID_PHASE" == loading && "$MID_W2" == true ]] \
   && [[ "$T49_N" -ge 1 ]] \
   && grep '"ev":"load_failed"' "$SV_EVENTS" | grep -q '"seat":2' \
   && grep -q '"ev":"round_void"' "$SV_EVENTS"; then
  pass "T49 load error: waited the retry window, then released; notice to clients; round void"
else fail "T49 load failure" "start=$FS mid=$MID_PHASE waiting2=$MID_W2 notices=$(s2g "$DIRS1" notice | tail -2 | head -c 400)"; fi
rcon_s "ABORT@400" >/dev/null
sleep 0.4

# T44: server --events: phase transitions + command results (JSONL)
if grep '"ev":"phase"' "$SV_EVENTS" | grep -q '"to":"Loading"' \
   && grep '"ev":"phase"' "$SV_EVENTS" | grep -q '"to":"Live"' \
   && grep '"ev":"phase"' "$SV_EVENTS" | grep '"to":"RoundOver"' | grep -q '"frozen_arena":"Map_Arena_Yard"' \
   && grep '"ev":"phase"' "$SV_EVENTS" | grep -q '"to":"Lobby"' \
   && grep '"ev":"cmd_result"' "$SV_EVENTS" | grep '"cmd_id":101' | grep -q '"ok":false' \
   && grep '"ev":"cmd_result"' "$SV_EVENTS" | grep -q '"source":"rcon"' \
   && grep -q '"ev":"server_start"' "$SV_EVENTS"; then
  pass "T44 server --events: phase Loading/Live/RoundOver(frozen_arena)/Lobby + cmd_result"
else fail "T44 server events" "$(head -c 800 $SV_EVENTS)"; fi
if grep '"ev":"cmd_result"' "$DIRS1/sidecar.jsonl" 2>/dev/null | grep -q '"cmd_id":101' \
   && grep '"ev":"phase"' "$DIRS1/sidecar.jsonl" | grep -q '"to":"live"'; then
  pass "T44b sidecar --events: cmd_result + phase"
else fail "T44b sidecar events" "$(head -c 600 $DIRS1/sidecar.jsonl 2>/dev/null)"; fi

# T45: --pid-file on the server (a hosting feature). The sidecar has no pid file:
# its game (the fake game ipc-game, $SS2, whose own parent is $WFAKE2, T46) spawned it,
# keeps its process handle, and sees it in the segment header (the attach in the game view).
if grep -q '"role":"server"' "$DIRSP/.pid.server.json" 2>/dev/null && grep -q '"pid":[0-9]' "$DIRSP/.pid.server.json" \
   && grep '"ev":"attach"' "$DIRS2/view.jsonl" 2>/dev/null | grep -q '"sidecar_pid":[1-9]' \
   && ! ls "$DIRS2"/.pid.* >/dev/null 2>&1 && ! "$BINS/hsmp-sidecar.exe" --help 2>&1 | grep -q -- '--pid-file'; then
  pass "T45 server pid file; the sidecar attached to its game (header pid), no sidecar pid file / flag"
else fail "T45 pid files" "$(cat $DIRSP/.pid.server.json 2>/dev/null) | $(grep '"ev":"attach"' $DIRS2/view.jsonl 2>/dev/null | head -c 300) | $(ls -a $DIRS2 2>&1 | tr '\n' ' ')"; fi

# T46: --parent-pid: the "game" dies -> the sidecar leaves and exits cleanly
kill $FAKE2 2>/dev/null
GONE=""
for i in 1 2 3 4 5 6 7 8; do
  sleep 0.5
  if ! kill -0 $SS2 2>/dev/null; then GONE=1; break; fi
done
one_row() { [[ "$(sess_nrows "$1")" -eq 1 ]]; }
rec_wait 3 one_row "$DIRS1"
if [[ "$GONE" == "1" ]] && grep -q "peer left" "$SCRATCH/t36_sv.log" && one_row "$DIRS1"; then
  pass "T46 parent died -> sidecar sent Leave and exited (seat freed at once)"
else fail "T46 parent-pid" "gone=$GONE $(sess_show "$DIRS1" 300)"; fi

# T47: a solo match whose only player leaves returns to the lobby (never stays live)
SOLO=$(rcon_s "START@400")
# S1 reported its game status in T49 (a game client now): report round 1 loaded (for this
# match: its id comes with the first snapshot of the match).
wait_phase 3 "$DIRS1" 'loading|countdown'
send_status "$DIRS1" 1 0 Map_Arena_Yard
LIVE_S=""
for i in 1 2 3 4 5 6 7 8; do
  sleep 0.5
  if rcon_s "STATUS@300" | grep -q '"phase":"live"'; then LIVE_S=1; break; fi
done
kill $FAKE1 2>/dev/null
sleep 2.0
ST_END=$(rcon_s "STATUS@400")
if echo "$SOLO" | grep -q "OK starting" && [[ "$LIVE_S" == "1" ]] && echo "$ST_END" | grep -q '"phase":"lobby"' \
   && echo "$ST_END" | grep -q '"peers":0'; then
  pass "T47 solo match: the only player left -> back to lobby"
else fail "T47 solo leave" "start=$SOLO live=$LIVE_S end=$ST_END"; fi

# T48: debug verbs are refused without --debug-verbs; SHUTDOWN removes the pid file
rcon_s "SHUTDOWN@400" >/dev/null
sleep 0.8
if [[ ! -f "$DIRSP/.pid.server.json" ]] && grep -q '"ev":"server_stop"' "$SV_EVENTS"; then
  pass "T48a server pid file removed on clean shutdown"
else fail "T48a server pidfile cleanup" "$(ls $DIRSP 2>&1)"; fi
e2e_port PORT_S2
e2e_port RCON_S2
"$BINS/hsmp-server.exe" --parent-pid "$E2E_WINPID" --bind "127.0.0.1:$PORT_S2" --tick-hz 30 \
  --rcon-bind "127.0.0.1:$RCON_S2" --rcon-password sesspw > "$SCRATCH/t48_sv.log" 2>&1 &
SVS2=$!
e2e_wait_bound $RCON_S2 $SVS2
DK=$("$HSMP_TOOLS" rcon "127.0.0.1:$RCON_S2" "AUTH sesspw@150" "DEBUG KILL 1@400")
if echo "$DK" | grep -q "ERR debug verbs are disabled"; then
  pass "T48b DEBUG KILL refused without --debug-verbs"
else fail "T48b debug verbs gate" "$DK"; fi
kill $SVS $SVS2 $SS1 $SS2 $FAKE1 $FAKE2 2>/dev/null; wait 2>/dev/null

# ============================================================================
hdr "ADM0-ADM9: Admin assignment (never the first joiner)"

# adm_sc DIR NICK PORT: a headless player (its own identity in DIR) joins 127.0.0.1:PORT.
adm_sc() {
  sc_launch "$1" --parent-pid "$E2E_WINPID" --server "127.0.0.1:$3" --state-dir "$1" \
    --nick "$2" > "$SCRATCH/adm_$2.log" 2>&1 &
}
# adm_bans DIR: the masked ban rows of DIR's `admin` record (one per line)
adm_bans() { rec_v "$1" admin | grep -o '"entry":"[^"]*"' | cut -d'"' -f4; }
e2e_port PORT_AD
e2e_port RCON_AD
DIRAD1="$SCRATCH/adm_stranger"; DIRAD2="$SCRATCH/adm_admin"
mkdir -p "${DIRAD1:?}" "${DIRAD2:?}"
# The configured admin's key comes from its own install (created on first use).
ADM_KEY=$(sc_launch "$DIRAD2" --print-player-key 2>/dev/null | tr -d '\r\n')
if [[ "$ADM_KEY" =~ ^[0-9a-f]{64}$ ]]; then
  pass "ADM0 hsmp-sidecar --print-player-key prints the player key"
else fail "ADM0 print-player-key" "$ADM_KEY"; fi
printf '# e2e admins file\n%s  # adm_admin\n' "$ADM_KEY" > "$SCRATCH/adm_admins.txt"
printf '203.0.113.77\n' > "$SCRATCH/adm_bans.txt"
"$BINS/hsmp-server.exe" --parent-pid "$E2E_WINPID" --bind "127.0.0.1:$PORT_AD" --tick-hz 30 --max-peers 4 \
  --admins-file "$SCRATCH/adm_admins.txt" --bans-file "$SCRATCH/adm_bans.txt" \
  --rcon-bind "127.0.0.1:$RCON_AD" --rcon-password admpw --events "$SCRATCH/adm_sv.jsonl" > "$SCRATCH/adm_sv.log" 2>&1 &
SVAD=$!
e2e_wait_bound $PORT_AD $SVAD && e2e_wait_bound $RCON_AD $SVAD
# ADM1: a stranger joins the dedicated server FIRST: not admin, no ban list, commands refused.
adm_sc "$DIRAD1" Stranger $PORT_AD; SCAD1=$!
wait_link 10 "$DIRAD1" connected
has_admin_rec() { [[ -n "$(rec "$1" admin)" ]]; }
rec_wait 5 has_admin_rec "$DIRAD1"
send_pick "$DIRAD1" 301 Pit
wait_cmd_result 5 "$DIRAD1" 301
if link_admin_is "$DIRAD1" false && [[ "$(jget "$(rec_v "$DIRAD1" admin)" admin_peer)" == 0 && -z "$(adm_bans "$DIRAD1")" ]] \
   && [[ "$(cmd_reason "$DIRAD1" 301)" == NOT_ADMIN ]]; then
  pass "ADM1 dedicated server: the first stranger is NOT admin (no bans sent, pick_arena NOT_ADMIN)"
else fail "ADM1 stranger admin" "$(link_show "$DIRAD1") | $(rec_v "$DIRAD1" admin) | $(cmd_result "$DIRAD1" 301)"; fi
# ADM2: the configured admin is admin; it sees the ban list masked, the stranger never.
adm_sc "$DIRAD2" Boss $PORT_AD; SCAD2=$!
wait_admin 10 "$DIRAD2" true
send_pick "$DIRAD2" 401 Pit
wait_cmd_result 5 "$DIRAD2" 401
if link_admin_is "$DIRAD2" true && cmd_ok "$DIRAD2" 401 true; then
  pass "ADM2 the --admins-file key is admin (pick_arena accepted)"
else fail "ADM2 configured admin" "$(link_show "$DIRAD2") | $(cmd_result "$DIRAD2" 401)"; fi
sleep 0.5
if adm_bans "$DIRAD2" | grep -q '^203\.0\.x\.x#[0-9a-f]\{8\}$' && ! rec_v "$DIRAD2" admin | grep -q '113\.77' \
   && ! rec_v "$DIRAD1" admin | grep -q '203\.0' && link_admin_is "$DIRAD1" false; then
  pass "ADM3 ban list: masked for the admin (203.0.x.x#token), absent for the stranger"
else fail "ADM3 ban privacy" "admin: $(rec_v "$DIRAD2" admin) | stranger: $(rec_v "$DIRAD1" admin)"; fi
BANS_AD=$("$HSMP_TOOLS" rcon "127.0.0.1:$RCON_AD" "AUTH admpw@200" "BANS@400")
if echo "$BANS_AD" | grep -q '203\.0\.113\.77'; then
  pass "ADM4 RCON BANS still lists full IPs"
else fail "ADM4 rcon bans" "$BANS_AD"; fi
# ADM5: RCON ADMIN ADD <peer id> grants the stranger admin (persisted to the admins file).
PID_AD1=$(link_get "$DIRAD1" my_peer_id)
ADD_AD=$("$HSMP_TOOLS" rcon "127.0.0.1:$RCON_AD" "AUTH admpw@200" "ADMIN ADD ${PID_AD1:-0}@500")
LIST_AD=$("$HSMP_TOOLS" rcon "127.0.0.1:$RCON_AD" "AUTH admpw@200" "ADMIN LIST@400")
wait_admin 5 "$DIRAD1" true
if echo "$ADD_AD" | grep -q "OK admin" && link_admin_is "$DIRAD1" true \
   && [[ "$(grep -c '^[0-9a-f]\{64\}' "$SCRATCH/adm_admins.txt")" -eq 2 ]] && echo "$LIST_AD" | grep -q "$ADM_KEY file online"; then
  pass "ADM5 RCON ADMIN ADD <peer_id> grants admin (written to --admins-file; ADMIN LIST)"
else fail "ADM5 rcon admin add" "$ADD_AD | $LIST_AD | $(link_show "$DIRAD1") | $(cat "$SCRATCH/adm_admins.txt")"; fi
kill $SVAD $SCAD1 $SCAD2 2>/dev/null; wait 2>/dev/null

# ADM6: listen server: the host (owner key file, written by its sidecar) is admin even
# though a stranger joined first.
e2e_port PORT_AH
DIRAH="$SCRATCH/adm_host"; DIRAS="$SCRATCH/adm_early"
mkdir -p "${DIRAH:?}" "${DIRAS:?}"
HSMP_LISTEN_HOST=1 "$BINS/hsmp-server.exe" --parent-pid "$E2E_WINPID" --bind "127.0.0.1:$PORT_AH" --tick-hz 30 --max-peers 4 \
  --owner-key-file "$DIRAH/.player_key" > "$SCRATCH/adm_listen_sv.log" 2>&1 &
SVAH=$!
e2e_wait_bound $PORT_AH $SVAH
adm_sc "$DIRAS" Early $PORT_AH; SCAS=$!
wait_link 10 "$DIRAS" connected
adm_sc "$DIRAH" Host $PORT_AH; SCAH=$!
wait_admin 10 "$DIRAH" true
# the host's roster row carries admin_role OWNER (3) in the stranger's snapshot
owner_row() { sess_rows "$1" | grep -q '"admin_role":3,'; }
rec_wait 5 owner_row "$DIRAS"
if link_admin_is "$DIRAH" true && link_admin_is "$DIRAS" false \
   && [[ "$(link_get "$DIRAS" my_peer_id)" == 1 ]] && owner_row "$DIRAS"; then
  pass "ADM6 listen server: the host's key is admin (OWNER) although a stranger joined first"
else fail "ADM6 listen host" "host: $(link_show "$DIRAH") | early: $(link_show "$DIRAS")"; fi
kill $SVAH $SCAS $SCAH 2>/dev/null; wait 2>/dev/null

# ADM7-ADM9: a dedicated server with NO admin still runs matches: everyone READY -> auto start.
e2e_port PORT_AN
DIRAN1="$SCRATCH/adm_n1"; DIRAN2="$SCRATCH/adm_n2"
mkdir -p "${DIRAN1:?}" "${DIRAN2:?}"
"$BINS/hsmp-server.exe" --parent-pid "$E2E_WINPID" --bind "127.0.0.1:$PORT_AN" --tick-hz 30 --max-peers 4 \
  --events "$SCRATCH/adm_noadmin.jsonl" > "$SCRATCH/adm_noadmin_sv.log" 2>&1 &
SVAN=$!
e2e_wait_bound $PORT_AN $SVAN
adm_sc "$DIRAN1" Nemo1 $PORT_AN; SCAN1=$!
adm_sc "$DIRAN2" Nemo2 $PORT_AN; SCAN2=$!
wait_link 10 "$DIRAN1" connected
wait_link 10 "$DIRAN2" connected
sleep 0.5
if link_admin_is "$DIRAN1" false && link_admin_is "$DIRAN2" false; then
  pass "ADM7 no admin configured: nobody is admin"
else fail "ADM7 no admin" "$(link_show "$DIRAN1") | $(link_show "$DIRAN2")"; fi
send_ready "$DIRAN1" 701
send_ready "$DIRAN2" 702
e2e_wait_file 5 "$SCRATCH/adm_noadmin.jsonl" '"state":"armed"'
wait_chat 3 "$DIRAN1" 'match starts in'
if grep '"ev":"auto_start"' "$SCRATCH/adm_noadmin.jsonl" | grep -q '"state":"armed"' && chat_has "$DIRAN1" 'match starts in'; then
  pass "ADM8 everyone READY with no admin: auto start armed (event + chat)"
else fail "ADM8 auto start armed" "$(tail -5 "$SCRATCH/adm_noadmin.jsonl" 2>/dev/null) | $(s2g "$DIRAN1" chat_in | tail -3 | head -c 300)"; fi
wait_phase 12 "$DIRAN1" 'loading|countdown|live'
if phase_is "$DIRAN1" 'loading|countdown|live' \
   && grep '"ev":"auto_start"' "$SCRATCH/adm_noadmin.jsonl" | grep -q '"state":"started"'; then
  pass "ADM9 no-admin server started the match from the ready players (no lobby deadlock)"
else fail "ADM9 auto start" "$(sess_show "$DIRAN1") | $(tail -5 "$SCRATCH/adm_noadmin.jsonl" 2>/dev/null)"; fi
kill $SVAN $SCAN1 $SCAN2 2>/dev/null; wait 2>/dev/null

# ============================================================================
hdr "T17: Master rate limiting (per-IP register throttle)"

e2e_port MPORT3
"$BINS/hsmp-master.exe" --parent-pid "$E2E_WINPID" --bind "127.0.0.1:$MPORT3" > "$SCRATCH/t17_mp.log" 2>&1 &
MP3=$!
e2e_wait_bound $MPORT3 $MP3

# Register once (should succeed)
R1=$(curl -s -o /dev/null -w "%{http_code}" -X POST "http://127.0.0.1:$MPORT3/v1/register" \
  -H "content-type: application/json" \
  -d '{"name":"RL Test","mode":"duel","host":"","port":7777,"players":1,"max_players":4,"password_required":false,"nonce":"n1","hmac":""}')
# Try to register from same IP again instantly — should be 429
R2=$(curl -s -o /dev/null -w "%{http_code}" -X POST "http://127.0.0.1:$MPORT3/v1/register" \
  -H "content-type: application/json" \
  -d '{"name":"Spam","mode":"ffa","host":"","port":7778,"players":1,"max_players":4,"password_required":false,"nonce":"n2","hmac":""}')
if [[ "$R1" == "201" && "$R2" == "429" ]]; then
  pass "T17 per-IP rate limit (first 201, second 429)"
else
  fail "T17 rate limit" "r1=$R1 r2=$R2"
fi

kill $MP3 2>/dev/null; wait 2>/dev/null

# ============================================================================
# Wire protocol section (docs/development/protocol.md; gate DoD-9/13). Headless
# protocol checks: handshake + identity, the DoD-9 attack probe (spoofed/replayed
# handshake and data packets, key material in cleartext, amplification),
# readable old-protocol rejects, seat replacement by key, terminal kick,
# server closing, and a short loadtest with zero decrypt failures.
hdr "W1-W8: wire protocol (handshake, DoD-9 probe, rejects, kick, closing)"

e2e_port PORTW
e2e_port RCONW
"$BINS/hsmp-server.exe" --parent-pid "$E2E_WINPID" --bind "127.0.0.1:$PORTW" --tick-hz 30 --max-peers 8 \
  --bans-file "$SCRATCH/w_bans.txt" --rcon-bind "127.0.0.1:$RCONW" --rcon-password e2epw \
  > "$SCRATCH/w_sv.log" 2>&1 &
SVW=$!
e2e_wait_bound $PORTW $SVW && e2e_wait_bound $RCONW $SVW
DIRW1="$SCRATCH/sideW1"; DIRW2="$SCRATCH/sideW2"; DIRW3="$SCRATCH/sideW3"; DIRW4="$SCRATCH/sideW4"
mkdir -p "${DIRW1:?}" "${DIRW2:?}" "${DIRW3:?}" "${DIRW4:?}"
sc_launch "$DIRW1" --parent-pid "$E2E_WINPID" --server "127.0.0.1:$PORTW" --state-dir "$DIRW1" --nick "Wire1" > "$SCRATCH/w_sc1.log" 2>&1 &
SW1=$!
sleep 1.5

# W1: handshake, per-install identity, TOFU record of the server key.
wait_link 5 "$DIRW1" connected
if link_is "$DIRW1" connected \
   && nocolor "$SCRATCH/w_sv.log" | grep -q "peer joined.*nick=Wire1.*player=" \
   && [[ "$(wc -c < "$DIRW1/identity" 2>/dev/null)" -eq 32 ]] \
   && grep -q "127.0.0.1:$PORTW" "$DIRW1/known_servers.json" 2>/dev/null; then
  pass "W1 v5_handshake: connected, 32-byte identity, server key recorded"
else
  fail "W1 v5_handshake" "$(link_show "$DIRW1") | $(grep 'peer joined' "$SCRATCH/w_sv.log" | tail -1)"
fi

# W2: the DoD-9 probe (hsmp-loadtest --v5-attack): one PASS line per check.
"$BINS/hsmp-loadtest.exe" --v5-attack --target "127.0.0.1:$PORTW" > "$SCRATCH/w_attack.log" 2>&1
for t in v5_junk_no_reply v5_hello_padded v5_response_le_request v5_no_key_in_clear v5_spoofed_hello \
         v5_replayed_auth_other_addr v5_spoof_allocates_no_peer v5_replayed_data v5_connection_survives_replays \
         v5_old_protocol_reject v4_join_outdated_reject; do
  line=$(grep -E "^(PASS|FAIL) $t:" "$SCRATCH/w_attack.log" | head -1)
  if [[ "$line" == PASS* ]]; then pass "W2 ${line#PASS }"
  else fail "W2 $t" "${line:-missing; see $SCRATCH/w_attack.log}"; fi
done

# W3: an old-protocol client (offers v3..=v4) gets the readable OUTDATED
# reject in its link record (status rejected, reason) and stays rejected (no reconnect loop).
sc_launch "$DIRW2" --parent-pid "$E2E_WINPID" --server "127.0.0.1:$PORTW" --state-dir "$DIRW2" --nick "OldMod" --proto-min 3 --proto-max 4 > "$SCRATCH/w_sc2.log" 2>&1 &
SW2=$!
sleep 3
if link_is "$DIRW2" rejected && link_get "$DIRW2" reason | grep -q "OUTDATED" \
   && [[ "$(grep -c 'reconnecting (new handshake' "$SCRATCH/w_sc2.log")" -le 1 ]]; then
  pass "W3 v5_version_reject: link reason says OUTDATED, status rejected, no retry"
else
  fail "W3 v5_version_reject" "$(link_show "$DIRW2")"
fi
kill $SW2 2>/dev/null

# W4: the same player key from a new socket replaces the old connection; that is a
# session resume (same peer id); the old client stops.
SC_IDDIR="$DIRW1" sc_launch "$DIRW3" --parent-pid "$E2E_WINPID" --server "127.0.0.1:$PORTW" --state-dir "$DIRW3" --nick "Wire1" > "$SCRATCH/w_sc3.log" 2>&1 &
SW3=$!
wait_link 5 "$DIRW3" connected
wait_link 3 "$DIRW1" replaced
W4_OLD_ID=$(link_get "$DIRW1" my_peer_id)
if nocolor "$SCRATCH/w_sv.log" | grep -q "session resumed" \
   && link_is "$DIRW3" connected \
   && [[ -n "$W4_OLD_ID" && "$(link_get "$DIRW3" my_peer_id)" == "$W4_OLD_ID" ]] \
   && link_is "$DIRW1" replaced; then
  pass "W4 v5_same_key_replaces: new socket took over, old client status=replaced"
else
  fail "W4 v5_same_key_replaces" "new: $(link_show "$DIRW3") | old: $(link_show "$DIRW1")"
fi
kill $SW1 2>/dev/null

# W5: an RCON kick is terminal: status stays kicked (reason in the link record), no rejoin.
W3_ID=$(link_get "$DIRW3" my_peer_id)
KICK_RESP=$("$HSMP_TOOLS" rcon "127.0.0.1:$RCONW" "AUTH e2epw@200" "KICK ${W3_ID:-0}@500")
sleep 5
if echo "$KICK_RESP" | grep -q "OK kicked" && link_is "$DIRW3" kicked \
   && [[ "$(grep -c 'welcome from relay' "$SCRATCH/w_sc3.log")" -eq 1 ]]; then
  pass "W5 v5_kick_terminal: kicked, no automatic rejoin after 5 s"
else
  fail "W5 v5_kick_terminal" "rcon: $KICK_RESP | $(link_show "$DIRW3") | welcomes $(grep -c 'welcome from relay' "$SCRATCH/w_sc3.log")"
fi
kill $SW3 2>/dev/null

# W6: RCON SHUTDOWN -> server_closing: the client shows server_closed
# with the reason within ~1 s instead of timing out.
sc_launch "$DIRW4" --parent-pid "$E2E_WINPID" --server "127.0.0.1:$PORTW" --state-dir "$DIRW4" --nick "Wire4" > "$SCRATCH/w_sc4.log" 2>&1 &
SW4=$!
wait_link 5 "$DIRW4" connected
"$HSMP_TOOLS" rcon "127.0.0.1:$RCONW" "AUTH e2epw@200" "SHUTDOWN@300" > /dev/null 2>&1
wait_link 3 "$DIRW4" server_closed
if link_is "$DIRW4" server_closed && link_get "$DIRW4" reason | grep -q "shut down"; then
  pass "W6 v5_server_closing: status server_closed, reason shown"
else
  fail "W6 v5_server_closing" "$(link_show "$DIRW4")"
fi
# W7: the server logged no AEAD failure during the whole section.
W_STATS=$(grep "v5 transport stats" "$SCRATCH/w_sv.log" | sed 's/\x1b\[[0-9;]*m//g')
if [[ -n "$W_STATS" ]] && ! echo "$W_STATS" | grep -v -q "aead_failed=0 "; then
  pass "W7 server: 0 AEAD failures under the attack probe"
else
  fail "W7 server AEAD failures" "$(echo "$W_STATS" | tail -1)"
fi
kill $SVW $SW4 2>/dev/null; wait 2>/dev/null

# W8: short loadtest (4 bots, 5 s): every bot connects, 0 decrypt failures.
e2e_port LT_PORT
"$BINS/hsmp-loadtest.exe" --server-bin "$BINS/hsmp-server.exe" --clients 4 --secs 5 --port $LT_PORT > "$SCRATCH/w_loadtest.log" 2>&1
if grep -q "RESULT: PASS" "$SCRATCH/w_loadtest.log"; then
  pass "W8 loadtest smoke: $(grep 'decrypt failures' "$SCRATCH/w_loadtest.log" | sed 's/^ *v5: //')"
else
  fail "W8 loadtest smoke" "$(tail -4 "$SCRATCH/w_loadtest.log")"
fi

# ============================================================================
# Reconnect section (DoD-11): reconnect (8 s blackout), host_leave, server_restart.
# Also runs on its own: HSMP_REPO=<tree> bash scripts/e2e-conn.sh
source "$REPO/scripts/e2e-conn.sh"

# ============================================================================
# NAT traversal (N1-N6): an emulated NAT in front of the host, a local STUN server and server
# list; a direct join fails, a punched join succeeds. Also runs on its own: scripts/e2e-nat.sh
source "$REPO/scripts/e2e-nat.sh"

# ============================================================================
# Shared-memory section: the shared-memory contracts checked directly (ipc-put / view), with
# `hsmp-tools ipc-game` as the game (docs/development/ipc-shared-memory.md).
source "$REPO/scripts/e2e-shm.sh"

# ============================================================================
# Server diagnostics: log files, the stats lines, RCON REPORT, --report.
source "$REPO/scripts/e2e-logs.sh"

# ============================================================================
# Cloudflare Worker server list (master-cf/) under wrangler dev, with the real server and
# hsmp-query; skipped without Node.js 22+, worker-build or the wasm32 target.
source "$REPO/scripts/e2e-master-cf.sh"

# ============================================================================
hdr "T15: Deploy script output artifacts in place"

# No game on this machine (CI, a contributor without Half Sword): nothing deployed to check.
if [[ ! -d "$GAME_MODS" ]]; then
  printf '  \e[33mSKIP\e[0m  %s\n' "T15: no deployed game at $GAME_MODS (set HSMP_GAME_DIR to check a deploy)"
else
  # Release profile (mods/mods.release.txt): the live mods are deployed and enabled;
  # the retired legacy mods (HSMPLobby/Admin/Character/Settings/Chat) are off.
  for m in HSMPSync HSMPAvatars HSMPMenu HSMPMatch HSMPCombat HSMPLoadout HSMPWorld HSMPHud HSMPNoCutscene; do
    p="$GAME_MODS/$m/Scripts/main.lua"
    if [[ -f "$p" ]]; then
      pass "T15 $m deployed ($(wc -c <"$p")B)"
    else
      fail "T15 $m deploy" "missing: $p"
    fi
  done

  T15_BAD=""
  for m in HSMPSync HSMPAvatars HSMPMenu HSMPMatch HSMPCombat HSMPLoadout HSMPWorld HSMPHud; do
    grep -qE "^$m : 1" "$GAME_MODS/mods.txt" 2>/dev/null || T15_BAD="$T15_BAD $m(off)"
  done
  for m in HSMPLobby HSMPAdmin HSMPCharacter HSMPSettings HSMPChat; do
    grep -qE "^$m : 1" "$GAME_MODS/mods.txt" 2>/dev/null && T15_BAD="$T15_BAD $m(on)"
  done
  if [[ -z "$T15_BAD" ]]; then
    pass "T15 mods.txt matches the release profile"
  else
    fail "T15 mods.txt" "wrong:$T15_BAD"
  fi
fi

# ============================================================================
echo
echo "============================================================"
echo "SUMMARY:  $PASS_COUNT passed, $FAIL_COUNT failed"
echo "============================================================"
if [[ $FAIL_COUNT -gt 0 ]]; then
  echo "Failed:"
  for n in "${FAIL_NAMES[@]}"; do echo "  - $n"; done
  exit 1
fi
exit 0
