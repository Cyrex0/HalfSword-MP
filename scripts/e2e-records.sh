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
REC_FLAG_LOADED=1; REC_FLAG_DEAD=4

# rec_wait TIMEOUT_S CMD...: poll CMD (a helper call) every 100 ms until it succeeds.
rec_wait() { local t="$1" i=0; shift; while (( i < t * 10 )); do "$@" && return 0; sleep 0.1; i=$((i+1)); done; return 1; }

# --- reading ------------------------------------------------------------------------------
# rec DIR SLOT: the newest slot line (empty if none yet); rec_v DIR SLOT: its record ("v") part.
rec() { grep -F "\"slot\":\"$2\"" "$1/view.jsonl" 2>/dev/null | tail -1; }
rec_v() { local l; l=$(rec "$1" "$2"); [[ -n "$l" ]] && printf '%s' "${l#*\"v\":}"; }
# jget JSON KEY: the first value of KEY (a number, bool or string, quotes stripped).
jget() { printf '%s' "$1" | grep -o "\"$2\":\(\"[^\"]*\"\|[^],}]*\)" | head -1 | cut -d: -f2- | sed 's/^"//; s/"$//'; }

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
# send_status DIR LOADED_ROUND DEAD(0|1) [ARENA] [LOAD_ERROR_NAME]: the Director's game
# status (the old `ping:<round>:<dead>:<arena>:<load_error>` verb), for the current match.
send_status() {
  local mid flags=0 le=0
  mid=$(sess_get "$1" match_id); [[ -z "$mid" ]] && mid=0
  (( $2 > 0 )) && flags=$((flags | REC_FLAG_LOADED))
  [[ "$3" == 1 ]] && flags=$((flags | REC_FLAG_DEAD))
  [[ -n "${5:-}" ]] && le=${REC_LOAD_ERROR[$5]:-9}
  rec_put "$1" game_status "{\"match_id\":$mid,\"round\":$2,\"flags\":$flags,\"load_error\":$le,\"arena\":\"${4:-}\"}"
}
