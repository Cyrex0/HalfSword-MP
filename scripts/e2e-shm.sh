# e2e-shm.sh: the shared-memory IPC section of scripts/e2e-test.sh
# (docs/development/ipc-shared-memory.md). Sourced by e2e-test.sh when HSMP_IPC=shm; it uses
# that script's helpers (pass/fail/hdr, e2e_port, e2e_wait_bound, e2e_wait_file) and
# variables (BINS, HSMP_TOOLS, SCRATCH, E2E_WINPID).
#
# The game is played by `hsmp-tools ipc-game`: it creates the segment as the game for its
# own PID, starts the REAL sidecar with `--parent-pid <its pid> --ipc shm:<name> --ipc-tap`,
# pumps the game side and logs what the game sees to view.jsonl. `hsmp-tools ipc-put`
# writes game-side slots / G2S messages given as JSON. Each contract is
# checked over shared memory once the sidecar negotiated its cap; a cap the sidecar does
# not offer is a FAILURE (there is no file fallback).

# A contract the sidecar does not carry over shared memory is a failure.
shm_skip() { fail "$1" "not available over shared memory (caps: $(shm_caps "$SHMA"))"; }
# shm_caps <dir>: the caps the sidecar negotiated (from the game view's attach line)
shm_caps() { grep -m1 '"ev":"attach"' "$1/view.jsonl" 2>/dev/null | grep -o '"caps_effective":\[[^]]*\]'; }
shm_has() { shm_caps "$1" | grep -q "\"$2\""; }
# shm_put <dir> <kind> <json>
shm_put() { "$HSMP_TOOLS" ipc-put --name "$(cat "$1/ipc.name")" "$2" "$3" >/dev/null; }
# shm_game <dir> <nick> <port>: start a fake game hosting a sidecar for <dir>
shm_game() {
  local d="$1" nick="$2" port="$3"
  HSMP_STATE_DIR="$d" "$HSMP_TOOLS" ipc-game --parent-pid "$E2E_WINPID" --name-file "$d/ipc.name" \
    --view "$d/view.jsonl" --tap "$d/ipc_tap.jsonl" -- \
    "$BINS/hsmp-sidecar.exe" --server "127.0.0.1:$port" --state-dir "$d" --nick "$nick" > "$d/game.log" 2>&1 &
}
# the typed-record helpers (link / session records, ipc-put; e2e-records.sh)
if ! declare -f rec >/dev/null 2>&1; then source "$REPO/scripts/e2e-records.sh"; fi
# shm_connected <dir>: the sidecar's `link` record says connected
shm_connected() { link_connected "$1"; }

hdr "S1-S12: shared-memory IPC (fake game: hsmp-tools ipc-game)"

e2e_port SHM_PORT
SHMA="$SCRATCH/shmA"; SHMB="$SCRATCH/shmB"
mkdir -p "${SHMA:?}" "${SHMB:?}"
"$BINS/hsmp-server.exe" --parent-pid "$E2E_WINPID" --bind "127.0.0.1:$SHM_PORT" --tick-hz 30 --max-peers 4 \
  --owner-key-file "$SHMA/.player_key" > "$SCRATCH/shm_sv.log" 2>&1 &
SHM_SV=$!
e2e_wait_bound $SHM_PORT $SHM_SV
shm_game "$SHMA" Alpha "$SHM_PORT"; SHM_GA=$!
for _ in $(seq 1 100); do shm_connected "$SHMA" && break; sleep 0.1; done
shm_game "$SHMB" Bravo "$SHM_PORT"; SHM_GB=$!
for _ in $(seq 1 100); do shm_connected "$SHMB" && break; sleep 0.1; done

# S1: attach handshake (ABI + layout hash + parent pid) and the tap
if e2e_wait_file 10 "$SHMA/view.jsonl" '"ev":"attach"' && e2e_wait_file 5 "$SHMB/view.jsonl" '"ev":"attach"'; then
  pass "S1 sidecars attached to the game segments (caps A: $(shm_caps "$SHMA"))"
else
  fail "S1 attach" "A view: $(head -c 300 "$SHMA/view.jsonl" 2>/dev/null) | A log: $(tail -c 300 "$SHMA/game.log" 2>/dev/null)"
fi
if grep -q '"ev":"refuse"' "$SHMA/view.jsonl" "$SHMB/view.jsonl" 2>/dev/null; then
  fail "S1 no refusal" "$(grep -h '"ev":"refuse"' "$SHMA/view.jsonl" "$SHMB/view.jsonl" | head -2)"
fi
if e2e_wait_file 5 "$SHMA/ipc_tap.jsonl" '"ev":"attach"'; then pass "S1b sidecar tap (ipc_tap.jsonl attach line)"
else fail "S1b tap" "$(head -c 300 "$SHMA/ipc_tap.jsonl" 2>/dev/null || echo missing)"; fi
if shm_connected "$SHMA" && shm_connected "$SHMB"; then pass "S2 both sidecars connected through the server"
else fail "S2 connected" "A: $(link_show "$SHMA") B: $(link_show "$SHMB")"; fi

# S3: the dump tool reads a live segment read-only
SHM_DUMP=$("$HSMP_TOOLS" ipc-dump --name "$(cat "$SHMA/ipc.name")" --get header --json 2>&1)
if echo "$SHM_DUMP" | grep -o '"sidecar":{.*' | grep -q '"state":"ready"'; then pass "S3 ipc-dump: sidecar ready in A's header"
else fail "S3 ipc-dump" "$(echo "$SHM_DUMP" | head -c 400)"; fi
# S3r: every ABI-2 record slot rendered by the schema (generic; an object even when empty)
SHM_REC=$("$HSMP_TOOLS" ipc-dump --name "$(cat "$SHMA/ipc.name")" --get records --json 2>&1)
if [[ "${SHM_REC:0:1}" == "{" ]]; then pass "S3r ipc-dump --get records (schema-driven record slots)"
else fail "S3r ipc-dump records" "$(echo "$SHM_REC" | head -c 300)"; fi
# S3d: DevCtl: ipc-ctl pushes dev_cmd records; the game's view renders them
"$HSMP_TOOLS" ipc-ctl --name "$(cat "$SHMA/ipc.name")" --id 77 autotest pick_arena Alley >/dev/null 2>&1
"$HSMP_TOOLS" ipc-ctl --name "$(cat "$SHMA/ipc.name")" tune servo 0.5 >/dev/null 2>&1
"$HSMP_TOOLS" ipc-ctl --name "$(cat "$SHMA/ipc.name")" tdiag on >/dev/null 2>&1
if e2e_wait_file 5 "$SHMA/view.jsonl" '"ev":"devctl".*"kind":"dev_cmd".*"tdiag"' \
   && grep '"ev":"devctl"' "$SHMA/view.jsonl" | grep '"key":"pick_arena"' | grep -q '"arg":"Alley"' \
   && grep '"ev":"devctl"' "$SHMA/view.jsonl" | grep '"key":"servo"' | grep -q '"num":0.5'; then
  pass "S3d ipc-ctl autotest / tune / tdiag -> dev_cmd records in the DevCtl ring"
else fail "S3d ipc-ctl" "$(grep '"ev":"devctl"' "$SHMA/view.jsonl" 2>/dev/null | head -c 500)"; fi
if ! "$HSMP_TOOLS" ipc-ctl --name "$(cat "$SHMA/ipc.name")" tdiag maybe >/dev/null 2>&1; then pass "S3e ipc-ctl refuses a malformed command"
else fail "S3e ipc-ctl validation" "tdiag maybe was accepted"; fi

if shm_has "$SHMA" POSE; then
  # S6 runs after S10 starts a match: lobby roots have no placed pawn generation.
  # S9: local pose frames A -> B's PeerPlay (playback evaluated by B's sidecar)
  for k in $(seq 1 12); do
    shm_put "$SHMA" local_pose "{\"tick\":$k,\"ts\":$((2000 + k * 16)),\"dt\":8.3,\"bones\":{\"pelvis\":[0,0,100,0,0,0,1,0,0,0,0,0,0],\"head\":[0,0,180,0,0,0,1,0,0,0,0,0,0],\"hand_r\":[30,10,140,0,0,0,1,0,0,0,0,0,0]}}"
    sleep 0.016
  done
  if e2e_wait_file 5 "$SHMB/view.jsonl" '"ev":"peer_play".*"pelvis":\[0\.0,0\.0,100\.0'; then pass "S9 pose over shm: A's pelvis reaches B's PeerPlay"
  else fail "S9 pose over shm" "$(grep '"ev":"peer_play"' "$SHMB/view.jsonl" | tail -1 | head -c 500)"; fi
  # S9n: no hot-stream files once POSE is negotiated
  SHM_FILES=$(ls -a "$SHMA" "$SHMB" 2>/dev/null | grep -E '^(me_[0-9]+\.json|me_remote[0-9]+\.json|\.skeletal\.json|\.weapon\.json|\.pose_play[0-9]+\.json)$' | tr '\n' ' ')
  if [[ -z "$SHM_FILES" ]]; then pass "S9n no pose/root/weapon state files with POSE over shm"
  else fail "S9n hot-stream files still written" "$SHM_FILES"; fi
else
  shm_skip "S6/S9: the sidecar did not negotiate POSE yet (pose/root still on files)"
fi

if shm_has "$SHMA" VITALS; then
  # The vitals record (quantised at the source: 87.5 hp = 5600, lowerarm_l = bit 11).
  shm_put "$SHMA" vitals '{"seq":1,"flags":2,"dism":2048,"v":[5600,6400,1600]}'
  if e2e_wait_file 5 "$SHMB/view.jsonl" '"ev":"slot".*"slot":"peer_vitals".*5600'; then pass "S9c vitals record over shm reaches B's peer_vitals slot"
  else fail "S9c vitals over shm" "$(grep '"slot":"peer_vitals"' "$SHMB/view.jsonl" | tail -1 | head -c 400)"; fi
else
  shm_skip "S9c: VITALS not negotiated yet"
fi

# SW1-SW6: world replication as typed records (schema/world.rs, protocol v6): the fake
# game writes the records with ipc-put (G2S world_sync / world_claim, slots world_out /
# world_manifest_out / world_hash); the sidecars' slots (world_owners / world_manifest /
# world_remote / world_consistency) show up as "slot" events in the game's view.
# shm_state <dir> <slot>: the newest "slot" line of that record slot in the game's view
shm_state() { grep "\"ev\":\"slot\"" "$1/view.jsonl" 2>/dev/null | grep "\"slot\":\"$2\"" | tail -1; }
# shm_wait_state <dir> <slot> <secs> <egrep pattern...>: until the newest line matches all
shm_wait_state() {
  local d="$1" k="$2" secs="$3"; shift 3
  local end=$((SECONDS + secs)) line p ok
  while (( SECONDS <= end )); do
    line=$(shm_state "$d" "$k"); ok=1
    for p in "$@"; do echo "$line" | grep -Eq "$p" || { ok=0; break; }; done
    [[ -n "$line" && $ok == 1 ]] && return 0
    sleep 0.1
  done
  return 1
}
if shm_has "$SHMA" WORLD; then
  shm_put "$SHMA" world_sync '{"level":777}'
  shm_put "$SHMB" world_sync '{"level":777}'
  if shm_wait_state "$SHMA" world_owners 5 '"sync":true' '"level":777' && shm_wait_state "$SHMB" world_owners 5 '"sync":true'; then
    pass "SW1 world_sync record -> world_owners (sync=true) on both games"
  else fail "SW1 world sync" "A: $(shm_state "$SHMA" world_owners | head -c 300) B: $(shm_state "$SHMB" world_owners | head -c 300)"; fi
  WEP=$(shm_state "$SHMA" world_owners | grep -o '"epoch":[0-9]*' | head -1 | cut -d: -f2); WEP=${WEP:-1}
  shm_put "$SHMA" world_manifest_out "{\"level\":777,\"epoch\":$WEP,\"rows\":[{\"id\":10,\"chash\":5,\"pos\":[100,50,10]},{\"id\":11,\"chash\":6,\"pos\":[400,50,10]}]}"
  if shm_wait_state "$SHMA" world_manifest 5 '"id":10' '"id":11' && shm_wait_state "$SHMB" world_manifest 5 '"id":10' '"id":11'; then
    pass "SW2 A's proposals -> the canonical manifest on both (proposal acked, broadcast to B)"
  else fail "SW2 manifest" "A: $(shm_state "$SHMA" world_manifest | head -c 300) B: $(shm_state "$SHMB" world_manifest | head -c 300)"; fi
  shm_put "$SHMA" world_out "{\"level\":777,\"epoch\":$WEP,\"seq\":1,\"ts\":100,\"rows\":[{\"id\":10,\"pos\":[110,50,10],\"rot\":[0,0,0],\"vel\":[5,0,0],\"flags\":200}]}"
  if shm_wait_state "$SHMB" world_remote 5 '"pos":\[110\.0,50\.0,10\.0\]' '"sender":1' && shm_wait_state "$SHMA" world_owners 5 '"owner":1'; then
    pass "SW3 A's world_state record reaches B's world_remote (sender 1); the implicit touch claim in the owners"
  else fail "SW3 world state" "B remote: $(shm_state "$SHMB" world_remote | head -c 400) A owners: $(shm_state "$SHMA" world_owners | head -c 300)"; fi
  shm_put "$SHMA" world_claim "{\"level\":777,\"epoch\":$WEP,\"req\":1,\"id\":10,\"mode\":0,\"has_rest\":true,\"rest\":{\"id\":10,\"pos\":[120,50,10],\"rot\":[0,0,0],\"vel\":[0,0,0],\"flags\":193}}"
  if shm_wait_state "$SHMB" world_remote 5 '"pos":\[120\.0,50\.0,10\.0\]' && shm_wait_state "$SHMB" world_owners 5 '"id":10,"mode":0,"owner":0'; then
    pass "SW4 release claim with its rest pose: free on B, the anchor (sender 0) in B's world_remote"
  else fail "SW4 release" "B remote: $(shm_state "$SHMB" world_remote | head -c 400) B owners: $(shm_state "$SHMB" world_owners | head -c 300)"; fi
  SW_HASH="{\"level\":777,\"epoch\":$WEP,\"seq\":1,\"hash\":1,\"rows\":[{\"id\":11,\"ver\":0,\"pos\":[400,50,10],\"q\":3221225472,\"status\":3}]}"
  shm_put "$SHMA" world_hash "$SW_HASH"
  sleep 0.3
  shm_put "$SHMB" world_hash "$SW_HASH"
  if shm_wait_state "$SHMB" world_consistency 5 '"hash_match":true' '"compared":1'; then
    pass "SW5 consistency: two world_hash reports -> a world_verdict (hash_match) in B's world_consistency"
  else fail "SW5 verdict" "$(shm_state "$SHMB" world_consistency | head -c 300)"; fi
  SW_FILES=$(ls -a "$SHMA" "$SHMB" 2>/dev/null | grep -E '^\.world.*\.(json|jsonl|req)$' | tr '\n' ' ')
  if [[ -z "$SW_FILES" ]]; then pass "SW6 no world state files (records over shared memory only)"
  else fail "SW6 world files written" "$SW_FILES"; fi
else
  shm_skip "SW1-SW6: WORLD not negotiated"
fi

if shm_has "$SHMA" LOADOUT_KIT; then
  # S9k: loadout domain records (schema loadout.rs). A's kit selection -> the server validates
  # it in place -> every game's peer_kit slot holds the verdict (A's own: the ack).
  shm_put "$SHMA" kit '{"seq":5,"class":"duelist","r":"w_arming3","l":"s_buckler3","cos":[1,2,3,0],"rows":[{"id":"h_hat2"},{"id":"b_arming"},{"id":"g_half2"},{"id":"l_hosen1"},{"id":"f_shoes1"}]}'
  if e2e_wait_file 5 "$SHMB/view.jsonl" '"ev":"slot".*"slot":"peer_kit".*"class":"duelist"'; then pass "S9k kit over shm: A's selection reaches B's peer_kit as the server's verdict"
  else fail "S9k kit over shm" "$(grep '"slot":"peer_kit"' "$SHMB/view.jsonl" | tail -2 | head -c 400)"; fi
  if e2e_wait_file 5 "$SHMA/view.jsonl" '"ev":"slot".*"slot":"peer_kit".*"class":"duelist".*"seq":5'; then pass "S9k2 A's own peer_kit acks seq 5 (resend stops)"
  else fail "S9k2 kit ack" "$(grep '"slot":"peer_kit"' "$SHMA/view.jsonl" | tail -2 | head -c 400)"; fi
  # a hostile selection (unknown class / weapon) is replaced by a class default
  shm_put "$SHMA" kit '{"seq":6,"class":"wizard","r":"w_lightsaber"}'
  if e2e_wait_file 5 "$SHMB/view.jsonl" '"ev":"slot".*"slot":"peer_kit".*"reason":"unknown class.*"verdict":1'; then pass "S9k3 an invalid kit is replaced by a class default (verdict 1 + reason)"
  else fail "S9k3 kit verdict" "$(grep '"slot":"peer_kit"' "$SHMB/view.jsonl" | tail -1 | head -c 400)"; fi
  # S9l: the appearance: ONE record per version (no chunks), A -> B's peer_loadout blob
  shm_put "$SHMA" loadout '{"version":77,"flags":1,"r":{"class":"@Weapons/Blueprints/Built_Weapons/BP_Sword","head_size":[1.0,1.0,1.0]},"rows":[{"slot":3,"flags":3,"class":"@Armor/X/BP_Torso","fabric1":[0.5,0.25,0.125,1.0],"rust":true,"pslot":3}]}'
  if e2e_wait_file 5 "$SHMB/view.jsonl" '"ev":"slot".*"slot":"peer_loadout".*"version":77'; then pass "S9l loadout over shm: A's record (v77) reaches B's peer_loadout"
  else fail "S9l loadout over shm" "$(grep '"slot":"peer_loadout"' "$SHMB/view.jsonl" | tail -1 | head -c 400)"; fi
  if grep '"ev":"slot".*"slot":"peer_loadout".*"version":77' "$SHMB/view.jsonl" | grep -q '"fabric1":\[0\.5,0\.25,0\.125,1\.0\]'; then pass "S9l2 passport fields arrive as they were written"
  else fail "S9l2 loadout fields" "$(grep '"slot":"peer_loadout"' "$SHMB/view.jsonl" | tail -1 | head -c 600)"; fi
  SHM_LO_FILES=$(ls -a "$SHMA" "$SHMB" 2>/dev/null | grep -E '^\.(kit|kit_rules|kit_rules\.out|loadout)\.json$|^\.(kit|loadout)_remote[0-9]+\.json$' | tr '\n' ' ')
  if [[ -z "$SHM_LO_FILES" ]]; then pass "S9n2 no kit / loadout state files"
  else fail "S9n2 kit/loadout files written" "$SHM_LO_FILES"; fi
else
  shm_skip "S9k: LOADOUT_KIT not negotiated"
fi

if shm_has "$SHMA" QUEUES && shm_has "$SHMA" STATE; then
  # S10: typed commands over the G2S ring (`command` records), answers over S2G
  # (`cmd_result`), the match state in the `session` record slot
  send_ready "$SHMA" 7001
  send_ready "$SHMB" 7002
  if rec_wait 5 has_cmd_result "$SHMA" 7001; then pass "S10a cmd_result over S2G for a G2S command"
  else fail "S10a cmd_result over shm" "$(grep '"ev":"s2g"' "$SHMA/view.jsonl" | tail -2 | head -c 400)"; fi
  # The server only starts once BOTH ready commands landed: wait for both in the session
  # record first (B's ready may still be in flight when A's cmd_result arrives), then start;
  # one retry covers a start that raced the roster update.
  shm_both_ready() { [[ "$(sess_rows "$1" | grep -c '"ready":true')" -eq 2 ]]; }
  rec_wait 5 shm_both_ready "$SHMA" || true
  send_cmd "$SHMA" 7003 start
  if ! wait_phase 3 "$SHMA" 'loading|countdown'; then
    send_cmd "$SHMA" 7004 start
  fi
  if wait_phase 5 "$SHMA" 'loading|countdown|live'; then pass "S10 ready+start over G2S -> countdown in the session record"
  else fail "S10 match over shm" "$(sess_show "$SHMA" 400)"; fi
  if grep -q '"ev":"g2s"' "$SHMA/ipc_tap.jsonl" 2>/dev/null; then pass "S10t G2S records in the tap"
  else fail "S10t tap g2s" "$(tail -c 300 "$SHMA/ipc_tap.jsonl" 2>/dev/null)"; fi
else
  shm_skip "S10: QUEUES/STATE not negotiated (commands and session records)"
fi

# S6: exact assigned root over shm after a healthy original-life placement report.
# No me_*.json is involved, and every context field must survive the relay.
if shm_has "$SHMA" POSE; then
  rec_wait 3 pawn_scope "$SHMA"
  send_loaded_status "$SHMA" 0 "$(sess_arena "$SHMA")"
  send_loaded_status "$SHMB" 0 "$(sess_arena "$SHMB")"
  pawn_scope "$SHMA"
  S6_ROOT=$(fixture_root "$SHMA" 42 "$REC_POS" 1000)
  shm_put "$SHMA" local_root "$S6_ROOT"
  if rec_wait 5 root_relayed "$SHMB" "$(link_get "$SHMA" my_peer_id)" "$S6_ROOT"; then
    pass "S6 root over shm: A's exact placed-life root reaches B's PeerRoot"
  else fail "S6 root over shm" "$(grep '"ev":"peer' "$SHMB/view.jsonl" | tail -3 | head -c 400)"; fi
fi

# S11: the game dies -> its sidecar leaves and exits (parent watch)
SHM_SC_A=$(grep -o '"sidecar_pid":[0-9]*' "$SHMA/view.jsonl" | head -1 | cut -d: -f2)
# Kill the game process ONLY, by its Windows PID: msys `kill` also terminates its child
# (the sidecar) on the spot, which would test a hard kill, not the parent watch.
SHM_GA_W=$(cat "/proc/$SHM_GA/winpid" 2>/dev/null)
if [[ -n "$SHM_GA_W" ]]; then taskkill //PID "$SHM_GA_W" //F >/dev/null 2>&1; else kill "$SHM_GA" 2>/dev/null; fi
SHM_GONE=0
for _ in $(seq 1 100); do
  if [[ -n "$SHM_SC_A" ]] && ! tasklist //FI "PID eq $SHM_SC_A" 2>/dev/null | grep -q "$SHM_SC_A"; then SHM_GONE=1; break; fi
  sleep 0.1
done
if [[ "$SHM_GONE" == 1 ]]; then pass "S11 game killed by PID -> its sidecar exited"
else fail "S11 sidecar outlived its game" "sidecar pid ${SHM_SC_A:-unknown}"; fi
if e2e_wait_file 3 "$SHMA/ipc_tap.jsonl" '"ev":"detach"'; then pass "S11t tap detach line"
else shm_skip "S11t: no detach line in A's tap"; fi

# S12: a sidecar given a bogus mapping refuses cleanly (exit 70..78), never hangs
"$BINS/hsmp-sidecar.exe" --parent-pid "$E2E_WINPID" --server "127.0.0.1:$SHM_PORT" --state-dir "$SCRATCH/shmX" \
  --nick X --ipc 'shm:Local\HSMP.ipc.1.0.does-not-exist' > "$SCRATCH/shm_x.log" 2>&1 &
SHM_X=$!
SHM_XC=""
for _ in $(seq 1 100); do if ! kill -0 $SHM_X 2>/dev/null; then wait $SHM_X; SHM_XC=$?; break; fi; sleep 0.1; done
if [[ -z "$SHM_XC" ]]; then kill $SHM_X 2>/dev/null; fail "S12 bogus mapping" "sidecar still running after 10 s"
elif (( SHM_XC != 0 )); then pass "S12 bogus --ipc mapping: sidecar exits with code $SHM_XC"
else fail "S12 bogus mapping" "exit code 0: $(tail -c 300 "$SCRATCH/shm_x.log")"; fi

kill "$SHM_GB" "$SHM_SV" 2>/dev/null
wait "$SHM_GA" "$SHM_GB" "$SHM_SV" 2>/dev/null
