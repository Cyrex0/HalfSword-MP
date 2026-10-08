# e2e-records.sh: the session domain's typed records in the e2e suites (see
# docs/development/ipc-shared-memory.md). Sourced by scripts/e2e-test.sh, e2e-conn.sh and
# e2e-shm.sh (needs HSMP_TOOLS). No JSON files: the game side of every sidecar is
# `hsmp-tools ipc-game`, which
#   - logs every sidecar-written record slot version to <dir>/view.jsonl:
#       {"ev":"slot","kind":"link","meta":{...},"slot":"link","t":..,"v":{record},"version":N}
#     (slots: link = the sidecar's status / admin flag / link state / kick-reject reason /
#     metrics, session = the server's snapshot, admin = the admin state), and every S2G
#     record: {"ev":"s2g","kind":"cmd_result"|"notice"|"chat_in"|"kill_feed",...,"v":{record}}.
#     serde_json renders object keys sorted; records use the Rust field names.
#   - writes its segment's mapping name to the --name-file the launchers pass, which
#     `hsmp-tools ipc-put` uses to push typed G2S records (command, game_status, spawned,
#     leave, chat) into that game's G2S ring.
# Every helper takes the sidecar's state dir first; its view is <dir>/view.jsonl and its
# mapping name <dir>/ipc_name.txt (sc_launch / q_sidecar), or <dir>/ipc.name (shm_game).

# Code tables (crates/hsmp-ipc/src/schema/session.rs ENUMS).
REC_STATUS=(connecting connected reconnecting rejected kicked replaced server_closed ended)
REC_LINK_STATE=(connecting up stalled reconnecting terminal)
REC_PHASE=(lobby loading countdown live roundover match_over post_match paused)
# the legacy (v4) match-state words of each phase, as shared/hsmp_session.lua STATE_OF_PHASE
REC_STATE=(lobby countdown countdown live roundover match_over lobby paused)
REC_REASON=(OK NOT_ADMIN WRONG_PHASE NOT_ALL_READY REV_MISMATCH UNKNOWN_ARENA INVALID_VALUE UNKNOWN_PLAYER NOT_ENOUGH_PLAYERS RATE_LIMITED UNSUPPORTED)
declare -A REC_OP=([ready]=1 [start]=2 [abort]=3 [pick_arena]=4 [set_config]=5 [kick]=6 [ban]=7 [promote]=8 [unban]=12 [reset_match]=13)
declare -A REC_LOAD_ERROR=([travel_failed]=1 [wrong_world]=2 [no_pawn]=3 [vitals]=4 [spawn_timeout]=5 [kit_error]=6 [stand_ins]=7 [timeout]=8)
REC_CFG_BEST_OF=4
REC_FLAG_LOADED=1; REC_FLAG_READY=2; REC_FLAG_DEAD=4

# rec_wait TIMEOUT_S CMD...: poll CMD (a helper call) every 100 ms until it succeeds.
rec_wait() { local t="$1" i=0; shift; while (( i < t * 10 )); do "$@" && return 0; sleep 0.1; i=$((i+1)); done; return 1; }

# --- reading ------------------------------------------------------------------------------
# rec DIR SLOT: the newest slot line (empty if none yet); rec_v DIR SLOT: its record ("v") part.
rec() { awk -v slot="$2" 'index($0,"\"slot\":\"" slot "\"") && /}\r?$/ { last=$0 } END { if (last!="") print last }' "$1/view.jsonl" 2>/dev/null; }
rec_v() { local l; l=$(rec "$1" "$2"); [[ -n "$l" ]] && printf '%s' "${l#*\"v\":}"; }
# jget JSON KEY: the first value of KEY (a number, bool or string, quotes stripped).
jget() { printf '%s' "$1" | grep -o "\"$2\":\(\"[^\"]*\"\|[^],}]*\)" | head -1 | cut -d: -f2- | sed 's/^"//; s/"$//'; }
jarray() { printf '%s' "$1" | grep -o "\"$2\":\[[^]]*\]" | head -1 | cut -d: -f2-; }

# The sidecar's link record.
link_get() { jget "$(rec_v "$1" link)" "$2"; }                        # DIR FIELD (my_peer_id, is_admin, reason, rtt_ms ...)
link_status() { local s; s=$(link_get "$1" status); [[ -n "$s" ]] && printf '%s' "${REC_STATUS[$s]:-?}"; }
link_is() { [[ "$(link_status "$1")" == "$2" ]]; }                     # DIR STATUS-NAME
link_state() { local s; s=$(link_get "$1" state); [[ -n "$s" ]] && printf '%s' "${REC_LINK_STATE[$s]:-?}"; }
link_admin_is() { [[ "$(link_get "$1" is_admin)" == "$2" ]]; }         # DIR true|false
link_connected() { link_is "$1" connected; }
wait_link() { rec_wait "$1" link_is "$2" "$3"; }                        # TIMEOUT DIR STATUS-NAME
wait_admin() { rec_wait "$1" link_admin_is "$2" "$3"; }                 # TIMEOUT DIR true|false
# a one-line summary for failure messages
link_show() { printf 'link{%s peer=%s admin=%s state=%s reason=%s}' "$(link_status "$1")" "$(link_get "$1" my_peer_id)" \
  "$(link_get "$1" is_admin)" "$(link_state "$1")" "$(link_get "$1" reason)"; }

# The server's session snapshot.
sess() { rec_v "$1" session; }
sess_get() { jget "$(sess "$1")" "$2"; }                               # DIR HEAD-FIELD (epoch, round, match_id, winner_seat ...)
sess_phase() { local p; p=$(sess_get "$1" phase); [[ -n "$p" ]] && printf '%s' "${REC_PHASE[$p]:-?}"; }
sess_state() { local p; p=$(sess_get "$1" phase); [[ -n "$p" ]] && printf '%s' "${REC_STATE[$p]:-?}"; }
phase_is() { [[ "$(sess_phase "$1")" =~ ^($2)$ ]]; }                    # DIR REGEX (e.g. 'countdown|live')
wait_phase() { rec_wait "$1" phase_is "$2" "$3"; }                      # TIMEOUT DIR REGEX
# roster rows (JSON objects; no nested objects inside a row)
sess_rows() { sess "$1" | grep -o '{[^{}]*"player_id":[^{}]*}'; }
sess_nrows() { sess_rows "$1" | grep -c '"player_id"'; }
row_of_peer() { sess_rows "$1" | grep -F "\"peer_id\":$2," | head -1; }  # DIR PEER_ID
row_of_seat() { sess_rows "$1" | grep -F "\"seat\":$2," | head -1; }     # DIR SEAT
my_seat() { local id; id=$(link_get "$1" my_peer_id); [[ -n "$id" ]] && jget "$(row_of_peer "$1" "$id")" seat; }
# the arena in force: the frozen config's in a match, else the lobby config's
sess_arena() {
  local s; s=$(sess "$1")
  if [[ "$(jget "$s" has_frozen)" == "true" && "$(jget "$s" phase)" != "0" ]]; then
    printf '%s' "$s" | grep -o '"frozen":{[^}]*}' | grep -o '"arena":"[^"]*"' | cut -d'"' -f4
  else
    printf '%s' "$s" | grep -o '"config":{[^}]*}' | grep -o '"arena":"[^"]*"' | cut -d'"' -f4
  fi
}
sess_show() { local s; s=$(sess "$1"); printf 'session{phase=%s round=%s arena=%s rows=%s} %s' "$(sess_phase "$1")" \
  "$(jget "$s" round)" "$(sess_arena "$1")" "$(sess_nrows "$1")" "${s:0:${2:-300}}"; }

# S2G records: s2g DIR KIND -> every such line.
s2g() { grep -F '"ev":"s2g"' "$1/view.jsonl" 2>/dev/null | grep -F "\"kind\":\"$2\""; }
cmd_result() { s2g "$1" cmd_result | grep -F "\"cmd_id\":$2," | tail -1; }       # DIR CMD_ID
cmd_results_n() { s2g "$1" cmd_result | grep -cF "\"cmd_id\":$2,"; }
cmd_reason() { local c; c=$(jget "$(cmd_result "$1" "$2")" reason_code); [[ -n "$c" ]] && printf '%s' "${REC_REASON[$c]:-?}"; }
cmd_ok() { [[ "$(jget "$(cmd_result "$1" "$2")" ok)" == "$3" ]]; }                  # DIR CMD_ID true|false
has_cmd_result() { [[ -n "$(cmd_result "$1" "$2")" ]]; }
wait_cmd_result() { rec_wait "$1" has_cmd_result "$2" "$3"; }                      # TIMEOUT DIR CMD_ID
chat_has() { s2g "$1" chat_in | grep -qF "$2"; }                                   # DIR TEXT
wait_chat() { rec_wait "$1" chat_has "$2" "$3"; }

# --- writing (typed G2S records through ipc-put) --------------------------------------------
rec_name_file() { if [[ -f "$1/ipc_name.txt" ]]; then printf '%s' "$1/ipc_name.txt"; else printf '%s' "$1/ipc.name"; fi; }
# rec_put DIR NAME JSON: one typed record (a game-written slot or G2S kind) into DIR's game.
rec_put() {
  local f i=0
  while :; do f=$(rec_name_file "$1"); [[ -s "$f" ]] && break; (( i++ > 50 )) && { echo "rec_put: no mapping name in $1" >&2; return 1; }; sleep 0.1; done
  "$HSMP_TOOLS" ipc-put --name "$(cat "$f")" "$2" "$3" >/dev/null
}
# send_cmd DIR CMD_ID OP [EXTRA_JSON_FIELDS]: a typed command (op name from REC_OP).
send_cmd() { rec_put "$1" command "{\"cmd_id\":$2,\"op\":${REC_OP[$3]}${4:+,$4}}"; }
send_ready() { send_cmd "$1" "$2" ready "\"flag\":${3:-true}"; }                    # DIR CMD_ID [true|false]
send_pick() { send_cmd "$1" "$2" pick_arena "\"text\":\"$3\""; }                    # DIR CMD_ID ARENA
send_chat() { rec_put "$1" chat "{\"text\":\"$2\"}"; }                             # DIR TEXT (no quotes)
send_leave() { rec_put "$1" leave '{"reason":0}'; }                                 # DIR (LeaveReason USER)
# Capture the fake pawn's original assignment once, before constructing its callback.
# Loading/countdown always constructs life 1; live generations must be observed in mode.
# No helper relabels a queued callback after the session changes.
pawn_scope() {
  local s m row mr p="${2:-$(link_get "$1" my_peer_id)}" phase
  s=$(sess "$1"); phase=$(jget "$s" phase)
  REC_MATCH=$(jget "$s" match_id)
  row=$(printf '%s' "$s" | grep -o '{[^{}]*"player_id":[^{}]*}' | grep -F "\"peer_id\":$p," | head -1)
  REC_SPAWN=$(jget "$row" spawn_id); REC_POS=$(jarray "$row" spawn_pos)
  [[ "$REC_MATCH" =~ ^[0-9]+$ && "$REC_SPAWN" =~ ^[0-9]+$ && -n "$REC_POS" ]] || return 1
  (( REC_MATCH > 0 && REC_SPAWN > 0 )) || return 1
  REC_ROUND=$((REC_SPAWN >> 8)); REC_LIFE=""
  m=$(rec_v "$1" mode)
  if [[ "$(jget "$m" match_id)" == "$REC_MATCH" && "$(jget "$m" round)" == "$REC_ROUND" ]]; then
    mr=$(printf '%s' "$m" | grep -o '{[^{}]*"peer_id":[^{}]*}' | grep -F "\"peer_id\":$p," | head -1)
    REC_LIFE=$(jget "$mr" life)
  elif [[ "$phase" == 1 || "$phase" == 2 ]] && (( REC_ROUND == $(jget "$s" round) + 1 )); then
    REC_LIFE=1
  fi
  [[ "$REC_LIFE" =~ ^[0-9]+$ ]] && (( REC_LIFE > 0 && REC_LIFE <= 65535 ))
}
fixture_root() { # DIR TICK POS [TS]: explicit scoped game callback, legacy root JSON shape
  pawn_scope "$1" || return 1
  printf '{"tick":%s,"ts":%s,"pos":%s,"rot":[0,0,0],"vel":[0,0,0],"match_id":%s,"round":%s,"life":%s}' \
    "$2" "${4:-0}" "$3" "$REC_MATCH" "$REC_ROUND" "$REC_LIFE"
}
# ipcgame::num renders source f32 values rounded to 1e-4; compare that exact
# display representation, not its spelling against the wider schema JSON number.
root_position_equal() {
  awk -v expected="$1" -v actual="$2" 'BEGIN {
    gsub(/[\[\]]/,"",expected); gsub(/[\[\]]/,"",actual)
    if (split(expected,e,",")!=3 || split(actual,a,",")!=3) exit 1
    for (i=1;i<=3;i++) {
      rounded=int(e[i]*10000+(e[i]<0 ? -0.5 : 0.5))/10000
      if (rounded != a[i]+0) exit 1
    }
  }'
}
root_relayed() { # DIR OWNER EXPECTED_ROOT: exact scoped content, not any historical root
  local r k
  r=$(grep '"ev":"peer_root"' "$1/view.jsonl" 2>/dev/null | grep -F "\"peer\":$2," | grep -F "\"tick\":$(jget "$3" tick)," | tail -1)
  [[ -n "$r" ]] && root_position_equal "$(jarray "$3" pos)" "$(jarray "$r" pos)" || return 1
  for k in match_id round life; do [[ "$(jget "$r" "$k")" == "$(jget "$3" "$k")" ]] || return 1; done
}
# The fake game must keep reporting while script/tool startup takes time. Capture
# this pawn's verified assignment once; never restamp the heartbeat into another life.
fixture_game() { # DIR ARENA; caller owns REC_GAME_PID and stops it at fixture teardown
  local d="$1" status
  pawn_scope "$d" || return 1
  status="{\"match_id\":$REC_MATCH,\"round\":$REC_ROUND,\"life\":$REC_LIFE,\"spawn_id\":$REC_SPAWN,\"world_key\":1,\"flags\":3,\"load_error\":0,\"arena\":\"$2\"}"
  rec_put "$d" game_status "$status" || return 1
  ( while sleep 1; do rec_put "$d" game_status "$status" || exit 1; done ) &
  REC_GAME_PID=$!
}
# The neutral skeleton from lagcomp/tests.rs::skeleton, with its linking v2 bones.
# The source sphere is the proven native_fist_sphere fixture: right hand, component 10,
# local center 13cm and radius 13cm. This is transport/geometry evidence, not game damage.
contact_pose() { # DIR TS DT ROOT_X ROOT_Y: pelvis Z 100, identity bone rotations
  pawn_scope "$1" || return 1
  awk -v mid="$REC_MATCH" -v round="$REC_ROUND" -v life="$REC_LIFE" -v ts="$2" -v dt="$3" -v x="$4" -v y="$5" '
    function bone(n,dx,dy,dz) { printf "%s\"%s\":[%.3f,%.3f,%.3f,0,0,0,1,0,0,0,0,0,0]", sep,n,x+dx,y+dy,100+dz; sep="," }
    BEGIN {
      printf "{\"tick\":1,\"ts\":%s,\"dt\":%s,\"context\":{\"match_id\":%s,\"round\":%s,\"life\":%s},\"bones\":{",ts,dt,mid,round,life
      bone("pelvis",0,0,0); bone("spine_01",0,0,12); bone("spine_02",0,0,25)
      bone("spine_03",0,0,37.5); bone("spine_04",0,0,50); bone("spine_05",0,0,62.5)
      bone("neck_01",0,0,66); bone("neck_02",0,0,70); bone("head",0,0,75)
      bone("clavicle_l",0,15,48); bone("upperarm_l",0,20,48); bone("lowerarm_l",20,25,40); bone("hand_l",40,20,40)
      bone("clavicle_r",0,-15,48); bone("upperarm_r",0,-20,48); bone("lowerarm_r",25,-25,40); bone("hand_r",45,-20,40)
      bone("thigh_l",0,10,-5); bone("calf_l",0,10,-50); bone("foot_l",0,10,-90)
      bone("thigh_r",0,-10,-5); bone("calf_r",0,-10,-50); bone("foot_r",0,-10,-90)
      printf "},\"strikers\":[{\"part\":1,\"component\":10,\"kind\":0,\"p\":[13,0,0],\"q\":[0,0,0,1],\"half\":[13,13,13]}]}"
    }'
}
send_loaded_status() { # DIR DEAD ARENA: apply this actual server spawn assignment
  pawn_scope "$1" || return 1
  send_status "$1" "$REC_ROUND" "$2" "$3"
}
send_death() { # DIR: a fresh original-life native death callback
  pawn_scope "$1" || return 1
  rec_put "$1" death_report "{\"match_id\":$REC_MATCH,\"round\":$REC_ROUND,\"life\":$REC_LIFE,\"reason\":0}"
}
# send_status DIR ROUND DEAD(0|1) [ARENA] [LOAD_ERROR_NAME]. A successful load applies
# the exact roster spawn; the fake game's world_key 1 represents its single verified world.
# A failure names the round which failed and never claims LOADED/placement.
send_status() {
  local mid flags=0 le=0 life=0 spawn=0 world=0
  mid=$(sess_get "$1" match_id); [[ -z "$mid" ]] && mid=0
  [[ -n "${5:-}" ]] && le=${REC_LOAD_ERROR[$5]:-9}
  if (( $2 > 0 && le == 0 )); then
    pawn_scope "$1" || return 1
    [[ "$REC_ROUND" == "$2" ]] || return 1
    mid=$REC_MATCH; life=$REC_LIFE; spawn=$REC_SPAWN; world=1
    flags=$((REC_FLAG_LOADED | REC_FLAG_READY))
  fi
  [[ "$3" == 1 ]] && flags=$((flags | REC_FLAG_DEAD))
  rec_put "$1" game_status "{\"match_id\":$mid,\"round\":$2,\"life\":$life,\"spawn_id\":$spawn,\"world_key\":$world,\"flags\":$flags,\"load_error\":$le,\"arena\":\"${4:-}\"}"
}
