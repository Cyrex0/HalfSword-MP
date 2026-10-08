#!/usr/bin/env bash
# e2e-conn.sh - connection-flow cases (gate DoD-11), headless:
#   Q1 reconnect       8 s blackout of one player mid-Live (hsmp-tools netsim
#                      --control-file): the duel pauses, the same seat and wins
#                      come back, the round is replayed and goes Live again.
#   Q2 host_leave      the listen host leaves (the game's typed `leave` record):
#                      the joiner shows server_closed "Host closed the server"
#                      within 5 s; a guest leaving a dedicated duel forfeits at once.
#   Q3 server_restart  kill + restart the server mid-Live: both clients
#                      reconnect on their own, see a NEW epoch and the lobby.
#
# Sourced by scripts/e2e-test.sh (uses its pass/fail/hdr, BINS, HSMP_TOOLS,
# SCRATCH and its per-run port helpers), or run on its own:
#   HSMP_TOOLS=<target>/release/hsmp-tools.exe HSMP_BINS=<target>/release bash scripts/e2e-conn.sh
# Every process started here is recorded at launch (Q_PIDS, never through a $(...)
# subshell) and stopped by that PID; long-lived children get --parent-pid of this
# shell; ports are allocated per run (e2e_port), never fixed.

if ! declare -f pass >/dev/null 2>&1; then
  set -u -o pipefail
  e2e_unix_path() { if command -v cygpath >/dev/null 2>&1; then cygpath -u "$1"; else printf '%s' "$1"; fi; }
  REPO="${HSMP_REPO:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
  E2E_TARGET="${CARGO_TARGET_DIR:+$(e2e_unix_path "$CARGO_TARGET_DIR")}"
  BINS="${HSMP_BINS:-${E2E_TARGET:-$REPO/target}/release}"
  HSMP_TOOLS="${HSMP_TOOLS:-${E2E_TARGET:-$REPO/target}/release/hsmp-tools.exe}"
  if [[ ! -x "$HSMP_TOOLS" ]]; then echo "hsmp-tools not found at $HSMP_TOOLS (set HSMP_TOOLS to the .exe)" >&2; exit 1; fi
  export HSMP_CAREER_GUARD=0
  E2E_WINPID=$(cat /proc/$$/winpid 2>/dev/null || echo $$)
  SCRATCH="${HSMP_SCRATCH:-${TMPDIR:-/tmp}/hsmp-e2e-conn-$$-$(date +%s)}"
  mkdir -p "$SCRATCH"
  export HSMP_STATE_DIR="$SCRATCH/home"; mkdir -p "$HSMP_STATE_DIR"
  PASS_COUNT=0; FAIL_COUNT=0; FAIL_NAMES=()
  pass() { PASS_COUNT=$((PASS_COUNT+1)); printf '  \e[32mPASS\e[0m  %s\n' "$1"; }
  fail() { FAIL_COUNT=$((FAIL_COUNT+1)); FAIL_NAMES+=("$1"); printf '  \e[31mFAIL\e[0m  %s\n' "$1" >&2; printf '        %s\n' "${2:-}" >&2; }
  hdr()  { printf '\n\e[36m== %s ==\e[0m\n' "$1"; }
  E2E_CONN_STANDALONE=1
fi
if ! declare -f e2e_port >/dev/null 2>&1; then
  # the same per-run port helpers as e2e-test.sh (kept in step with it)
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
fi

# the typed-record helpers (link / session records, cmd_result / notice events, ipc-put)
if ! declare -f rec >/dev/null 2>&1; then source "$REPO/scripts/e2e-records.sh"; fi

Q_PIDS=()
# Extra hsmp-server args for the next q_server (e.g. the listen host's --owner-key-file).
Q_EXTRA=()
Q_LAST=""
q_stop() { for p in "$@"; do [[ -n "$p" ]] && kill "$p" 2>/dev/null; done; }
q_cleanup() { q_stop "${Q_PIDS[@]}"; wait 2>/dev/null; Q_PIDS=(); }
if [[ "${E2E_CONN_STANDALONE:-0}" == 1 ]]; then
  # a failed or interrupted standalone run stops what it started (by recorded PID only)
  trap 'q_cleanup; e2e_release_ports' EXIT
fi

# A headless "game": reports its game status like HSMPMatch's Director (a typed
# `game_status` record: the current assigned round/life loaded on <arena>, alive) every second, so the server
# treats the client as a game in its load barrier.
q_game() {   # dir arena
  local d="$1" a="$2"
  ( while true; do
      if pawn_scope "$d"; then send_loaded_status "$d" 0 "$a"; else send_status "$d" 0 0; fi
      sleep 1
    done ) &
  Q_PIDS+=($!)
}
# q_sidecar / q_server run in THIS shell (never call them inside $(...), the PID would be
# recorded in a subshell and lost); the started PID is in $Q_LAST.
# The sidecar runs behind a fake game (`hsmp-tools ipc-game`): shared-memory IPC only; the
# checks below read the typed records the game sees (view.jsonl: link / session
# slots, notice events) and write typed records with ipc-put (e2e-records.sh).
# Q_LAST is the fake game, which exits with the sidecar's exit code.
q_sidecar() {   # dir server nick log
  rm -f "$1/view.jsonl" "$1/ipc_name.txt"
  HSMP_STATE_DIR="$1" "$HSMP_TOOLS" ipc-game --parent-pid "$E2E_WINPID" --bridge "$1" --view "$1/view.jsonl" \
    --name-file "$1/ipc_name.txt" --tap "$1/ipc_tap.jsonl" -- "$BINS/hsmp-sidecar.exe" --server "$2" --state-dir "$1" --nick "$3" \
    --events "$1/sidecar.jsonl" > "$4" 2>&1 &
  Q_LAST=$!
  Q_PIDS+=($Q_LAST)
}
q_server() {   # port rcon events log [env...]
  local port="$1" rcon="$2" ev="$3" log="$4"; shift 4
  env "$@" "$BINS/hsmp-server.exe" --bind "127.0.0.1:$port" --tick-hz 30 --max-peers 4 \
    --rcon-bind "127.0.0.1:$rcon" --rcon-password qpw --debug-verbs --events "$ev" --parent-pid "$E2E_WINPID" "${Q_EXTRA[@]}" > "$log" 2>&1 &
  Q_LAST=$!
  Q_PIDS+=($Q_LAST)
  e2e_wait_bound "$port" "$Q_LAST" && e2e_wait_bound "$rcon" "$Q_LAST"
}
q_rcon() { local r="$1"; shift; "$HSMP_TOOLS" rcon "127.0.0.1:$r" "AUTH qpw@150" "$@"; }
# both ready, MAP Alley, START, wait for Live
q_start_match() {   # rcon d1 d2
  send_ready "$2" 9001
  send_ready "$3" 9002
  sleep 1.0
  q_rcon "$1" "MAP Alley@300" >/dev/null
  q_rcon "$1" "START@400" >/dev/null
  wait_phase 20 "$2" live
}
# q_wins DIR SEAT: that seat's wins in DIR's session snapshot
q_wins() { jget "$(row_of_seat "$1" "$2")" wins; }

# ============================================================================
hdr "Q1: reconnect - 8 s blackout mid-Live: paused, same seat + wins, Live again (DoD-11)"
e2e_port QP; e2e_port QR; e2e_port QN
QD1="$SCRATCH/q1a"; QD2="$SCRATCH/q1b"; mkdir -p "$QD1" "$QD2"
QCTL="$SCRATCH/q1_netsim.ctl"; echo normal > "$QCTL"
q_server $QP $QR "$SCRATCH/q1_sv.jsonl" "$SCRATCH/q1_sv.log"
"$HSMP_TOOLS" netsim --listen "127.0.0.1:$QN" --upstream "127.0.0.1:$QP" --delay 1 --jitter 0 --loss 0 --dup 0 \
  --control-file "$QCTL" --parent-pid "$E2E_WINPID" > "$SCRATCH/q1_netsim.log" 2>&1 &
Q_PIDS+=($!)
e2e_wait_bound $QN
q_sidecar "$QD1" "127.0.0.1:$QP" "Stayer" "$SCRATCH/q1_s1.log"
sleep 0.6
q_sidecar "$QD2" "127.0.0.1:$QN" "Dropper" "$SCRATCH/q1_s2.log"
sleep 1.5
q_game "$QD1" Map_Arena_Alley; q_game "$QD2" Map_Arena_Alley
if q_start_match $QR "$QD1" "$QD2"; then pass "Q1a match live (two reporting game clients)"
else fail "Q1a match live" "$(sess_show "$QD1" 400)"; fi
# A complete own roster row must have arrived before freezing the reconnect baseline.
q1_has_seat() { local s; s=$(my_seat "$1"); [[ "$s" =~ ^[1-9][0-9]*$ ]]; }
rec_wait 5 q1_has_seat "$QD2"
SEAT2=$(my_seat "$QD2")
# Dropper wins round 1 (kill the Stayer's seat) so there are wins to keep.
SEAT1=$(my_seat "$QD1")
sleep 3.2   # past the spawn protection
q_rcon $QR "DEBUG KILL ${SEAT1:-1}@400" >/dev/null
round2_live() { phase_is "$1" live && [[ "$(sess_get "$1" round)" == 2 ]]; }
rec_wait 20 round2_live "$QD1"
WINS_BEFORE=$(q_wins "$QD1" "${SEAT2:-2}")
B0=$(date +%s)
echo blackout > "$QCTL"
sleep 5
LINK_MID=$(link_state "$QD2")
PAUSED_MID=$(sess_phase "$QD1")
sleep 3
echo normal > "$QCTL"
if [[ "$PAUSED_MID" == paused ]]; then pass "Q1b the duel paused during the blackout"
else fail "Q1b paused" "phase $PAUSED_MID"; fi
if [[ "$LINK_MID" == stalled || "$LINK_MID" == reconnecting ]]; then pass "Q1c the dropper's link record said $LINK_MID (Reconnecting overlay)"
else fail "Q1c link state" "$LINK_MID | $(link_show "$QD2")"; fi
if wait_phase 40 "$QD1" live; then pass "Q1d back to Live after the blackout ($(( $(date +%s) - B0 )) s after it began)"
else fail "Q1d live again" "$(sess_show "$QD1" 400)"; fi
# The dropper's link / roster records land a few ticks after Live: let them converge (3 s).
q1_seat_back() { [[ -n "$SEAT2" && "$(my_seat "$QD2")" == "$SEAT2" && "$(q_wins "$QD1" "$SEAT2")" == "$WINS_BEFORE" ]]; }
rec_wait 3 q1_seat_back
SEAT2B=$(my_seat "$QD2")
WINS_AFTER=$(q_wins "$QD1" "${SEAT2B:-0}")
if [[ -n "$SEAT2" && "$SEAT2" == "$SEAT2B" && -n "$WINS_BEFORE" && "$WINS_BEFORE" == "$WINS_AFTER" && "${WINS_AFTER:-0}" -ge 1 ]]; then
  pass "Q1e same seat ($SEAT2) and wins ($WINS_AFTER) after the blackout"
else fail "Q1e seat/wins" "seat $SEAT2 -> $SEAT2B, wins $WINS_BEFORE -> $WINS_AFTER"; fi
if grep -q '"ev":"seat_restored"\|"event":"seat_restored"\|seat_restored' "$SCRATCH/q1_sv.jsonl" 2>/dev/null; then
  pass "Q1f server event seat_restored"
else fail "Q1f seat_restored event" "$(grep -c . "$SCRATCH/q1_sv.jsonl" 2>/dev/null) events"; fi
q_cleanup

# ============================================================================
hdr "Q2: host_leave - the listen host leaves: joiner sees 'Host closed the server' within 5 s"
e2e_port QP; e2e_port QR
QD1="$SCRATCH/q2host"; QD2="$SCRATCH/q2join"; mkdir -p "$QD1" "$QD2"
# Security: the host is the listen server's owner by its player key (as HSMPMenu HOST passes it).
Q_EXTRA=(--owner-key-file "$QD1/.player_key")
q_server $QP $QR "$SCRATCH/q2_sv.jsonl" "$SCRATCH/q2_sv.log" HSMP_LISTEN_HOST=1
Q_EXTRA=()
q_sidecar "$QD1" "127.0.0.1:$QP" "Host" "$SCRATCH/q2_s1.log"; HOSTPID=$Q_LAST
sleep 0.6
q_sidecar "$QD2" "127.0.0.1:$QP" "Joiner" "$SCRATCH/q2_s2.log"
sleep 1.5
q_game "$QD1" Map_Arena_Alley; q_game "$QD2" Map_Arena_Alley
q_start_match $QR "$QD1" "$QD2" >/dev/null
T0=$(date +%s%N)
# the host presses CANCEL: the game's typed `leave` record
send_leave "$QD1"
if wait_link 5 "$QD2" server_closed; then
  DT=$(( ($(date +%s%N) - T0) / 1000000 ))
  pass "Q2a joiner status server_closed after ${DT} ms"
else fail "Q2a joiner closed" "$(link_show "$QD2")"; fi
if link_get "$QD2" reason | grep -q "Host closed the server"; then pass "Q2b reason: Host closed the server"
else fail "Q2b reason" "$(link_show "$QD2")"; fi
sleep 1.5
if link_is "$QD1" ended && ! kill -0 "$HOSTPID" 2>/dev/null; then
  pass "Q2c the host's sidecar left and exited cleanly (status ended)"
else fail "Q2c host sidecar exit" "$(link_show "$QD1")"; fi
q_cleanup
# Q2d: a guest leaving a DEDICATED duel on purpose: forfeit at once, no 30 s pause
e2e_port QP; e2e_port QR
QD1="$SCRATCH/q2d_a"; QD2="$SCRATCH/q2d_b"; mkdir -p "$QD1" "$QD2"
q_server $QP $QR "$SCRATCH/q2d_sv.jsonl" "$SCRATCH/q2d_sv.log"
q_sidecar "$QD1" "127.0.0.1:$QP" "Admin" "$SCRATCH/q2d_s1.log"
sleep 0.6
q_sidecar "$QD2" "127.0.0.1:$QP" "Quitter" "$SCRATCH/q2d_s2.log"
sleep 1.5
q_game "$QD1" Map_Arena_Alley; q_game "$QD2" Map_Arena_Alley
q_start_match $QR "$QD1" "$QD2" >/dev/null
send_leave "$QD2"
# result_reason FORFEIT = 3; the snapshots in between never showed paused
if wait_phase 4 "$QD1" match_over && [[ "$(sess_get "$QD1" result_reason)" == 3 ]] \
   && ! grep -F '"slot":"session"' "$QD1/view.jsonl" | grep -q '"phase":7,'; then
  pass "Q2d a deliberate leave forfeits at once (match_over/forfeit, no pause)"
else fail "Q2d deliberate leave" "$(sess_show "$QD1" 400)"; fi
# the PLAYER_LEFT notice (code 5) with args [nick, "left"]: the HUD toast "Quitter left"
if s2g "$QD1" notice | grep '"code":5,' | grep -q '"args":\["Quitter","left"'; then
  pass "Q2e notice PLAYER_LEFT 'Quitter left' for the HUD toast"
else fail "Q2e notice" "$(s2g "$QD1" notice | tail -3 | head -c 400)"; fi
q_cleanup

# ============================================================================
hdr "Q3: server_restart - kill + restart mid-Live: new epoch, lobby, reconnected by themselves"
e2e_port QP; e2e_port QR
QD1="$SCRATCH/q3a"; QD2="$SCRATCH/q3b"; mkdir -p "$QD1" "$QD2"
q_server $QP $QR "$SCRATCH/q3_sv.jsonl" "$SCRATCH/q3_sv1.log"; SVQ=$Q_LAST
q_sidecar "$QD1" "127.0.0.1:$QP" "One" "$SCRATCH/q3_s1.log"
sleep 0.6
q_sidecar "$QD2" "127.0.0.1:$QP" "Two" "$SCRATCH/q3_s2.log"
sleep 1.5
q_game "$QD1" Map_Arena_Alley; q_game "$QD2" Map_Arena_Alley
q_start_match $QR "$QD1" "$QD2" >/dev/null
# the session epoch (the server instance) of DIR's newest snapshot
q_epoch() { sess_get "$1" epoch; }
E1=$(q_epoch "$QD1")
kill "$SVQ" 2>/dev/null; wait "$SVQ" 2>/dev/null
sleep 1
q_server $QP $QR "$SCRATCH/q3_sv2.jsonl" "$SCRATCH/q3_sv2.log"
new_epoch_lobby() { phase_is "$1" lobby && [[ -n "$(q_epoch "$1")" && "$(q_epoch "$1")" != "$E1" ]]; }
OK3=1
for d in "$QD1" "$QD2"; do
  rec_wait 40 new_epoch_lobby "$d" || OK3=0
done
E1B=$(q_epoch "$QD1")
E2B=$(q_epoch "$QD2")
if [[ "$OK3" == 1 && -n "$E1" && -n "$E1B" && "$E1B" != "$E1" && "$E1B" == "$E2B" ]]; then
  pass "Q3a both clients reconnected on their own: new epoch ($E1 -> $E1B), lobby"
else fail "Q3a restart" "epoch $E1 -> $E1B / $E2B | $(sess_show "$QD1" 200)"; fi
if wait_link 3 "$QD1" connected && wait_link 3 "$QD2" connected; then
  pass "Q3b sidecars connected again (no terminal state, no manual rejoin)"
else fail "Q3b connected" "$(link_show "$QD1") | $(link_show "$QD2")"; fi
q_cleanup

if [[ "${E2E_CONN_STANDALONE:-0}" == 1 ]]; then
  echo
  echo "e2e-conn: $PASS_COUNT passed, $FAIL_COUNT failed"
  for n in "${FAIL_NAMES[@]}"; do echo "  - $n"; done
  [[ $FAIL_COUNT -eq 0 ]]
fi
