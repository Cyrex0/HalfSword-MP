#!/usr/bin/env bash
# e2e-nat.sh - NAT traversal, headless, all on 127.0.0.1 (no router, no public server):
#   N1  the host learns its public endpoint with STUN from the game socket (a local STUN
#       server: hsmp-tools natlab stun), lists itself as "cone" and opens the punch relay
#       socket to the server list (hsmp-master)
#   N2  behind the emulated NAT (hsmp-server --emulate-nat: endpoint-independent mapping,
#       address-and-port-dependent filtering) a DIRECT join fails: every Hello is dropped
#   N3  the same join with NAT traversal succeeds: the joiner's sidecar learns its public
#       endpoint (STUN on its game socket), posts a punch request, the host probes that
#       endpoint, the next Hello passes and the handshake completes as usual
#   N4  the probes go exactly to the endpoint asked for (hsmp-tools natlab catch), 4 small
#       datagrams
#   N5  the relay refuses an endpoint that is not the requester's address, ports < 1024,
#       unknown servers, and throttles a flood
#   N6  a host nothing can reach: the joiner is told to ask for a forwarded UDP port
#
# Sourced by scripts/e2e-test.sh (uses its pass/fail/hdr, sc_launch, BINS, HSMP_TOOLS,
# SCRATCH, the port helpers and the record helpers), or on its own:
#   HSMP_TOOLS=<target>/release/hsmp-tools.exe HSMP_BINS=<target>/release bash scripts/e2e-nat.sh

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
  SCRATCH="${HSMP_SCRATCH:-${TMPDIR:-/tmp}/hsmp-e2e-nat-$$-$(date +%s)}"
  mkdir -p "$SCRATCH"
  export HSMP_STATE_DIR="$SCRATCH/home"; mkdir -p "$HSMP_STATE_DIR"
  PASS_COUNT=0; FAIL_COUNT=0; FAIL_NAMES=()
  pass() { PASS_COUNT=$((PASS_COUNT+1)); printf '  \e[32mPASS\e[0m  %s\n' "$1"; }
  fail() { FAIL_COUNT=$((FAIL_COUNT+1)); FAIL_NAMES+=("$1"); printf '  \e[31mFAIL\e[0m  %s\n' "$1" >&2; printf '        %s\n' "${2:-}" >&2; }
  hdr()  { printf '\n\e[36m== %s ==\e[0m\n' "$1"; }
  E2E_NAT_STANDALONE=1
fi
if ! declare -f e2e_port >/dev/null 2>&1; then
  # a free TCP+UDP port (no cross-run reservation in standalone mode)
  e2e_port() {
    local __var="$1" p i
    for i in $(seq 1 200); do
      p=$(( 20000 + ( (RANDOM << 15) | RANDOM ) % 29000 ))
      netstat -ano 2>/dev/null | awk -v p="$p" '($1=="TCP"||$1=="UDP") { n=split($2,a,":"); if (a[n]==p) f=1 } END { exit !f }' && continue
      printf -v "$__var" '%s' "$p"; return 0
    done
    return 1
  }
  e2e_wait_bound() {
    local port="$1" pid="${2:-}" t="${3:-15}" i=0
    while (( i < t * 10 )); do
      netstat -ano 2>/dev/null | awk -v a="127.0.0.1:$port" '$2==a && ($1=="UDP" || $4=="LISTENING") { f=1 } END { exit !f }' && return 0
      [[ -n "$pid" ]] && ! kill -0 "$pid" 2>/dev/null && return 1
      sleep 0.1; i=$((i+1))
    done
    return 1
  }
fi
if ! declare -f sc_launch >/dev/null 2>&1; then
  # the real sidecar behind a fake game (as e2e-test.sh's sc_launch)
  sc_launch() {
    local d="$1"; shift
    local pp=() args=()
    while [[ $# -gt 0 ]]; do
      case "$1" in
        --parent-pid) pp=(--parent-pid "$2"); shift 2 ;;
        *) args+=("$1"); shift ;;
      esac
    done
    rm -f "$d/ipc_name.txt" "$d/view.jsonl"
    HSMP_STATE_DIR="${SC_IDDIR:-$d}" exec "$HSMP_TOOLS" ipc-game "${pp[@]}" --bridge "$d" --name-file "$d/ipc_name.txt" \
      --view "$d/view.jsonl" --tap "$d/ipc_tap.jsonl" -- "$BINS/hsmp-sidecar.exe" "${args[@]}"
  }
fi
if ! declare -f rec >/dev/null 2>&1; then source "$REPO/scripts/e2e-records.sh"; fi

hdr "N1-N6: NAT traversal (emulated NAT in front of the host, local STUN and server list)"

N_PIDS=()
n_stop() { local p; for p in "${N_PIDS[@]}"; do kill "$p" 2>/dev/null; done; wait 2>/dev/null; N_PIDS=(); }
nlog() { sed 's/\x1b\[[0-9;]*m//g' "$1" 2>/dev/null; }

e2e_port N_GAME; e2e_port N_MASTER; e2e_port N_STUN; e2e_port N_CATCH
NB="http://127.0.0.1:$N_MASTER"
"$HSMP_TOOLS" natlab stun --bind "127.0.0.1:$N_STUN" --secs 300 > "$SCRATCH/n_stun.log" 2>&1 &
N_PIDS+=($!)
"$BINS/hsmp-master.exe" --parent-pid "$E2E_WINPID" --bind "127.0.0.1:$N_MASTER" > "$SCRATCH/n_master.log" 2>&1 &
N_PIDS+=($!); N_MP=$!
e2e_wait_bound "$N_MASTER" "$N_MP"
HSMP_MASTER_URL="$NB" HSMP_STUN_SERVERS="127.0.0.1:$N_STUN" "$BINS/hsmp-server.exe" --parent-pid "$E2E_WINPID" \
  --bind "127.0.0.1:$N_GAME" --tick-hz 30 --name "NAT Host" --port-map off --emulate-nat > "$SCRATCH/n_sv.log" 2>&1 &
N_PIDS+=($!); N_SV=$!
e2e_wait_bound "$N_GAME" "$N_SV"

# N1: listed as "cone", relay open
NL=""
for i in $(seq 1 60); do
  NL=$(curl -s "$NB/v1/servers")
  echo "$NL" | grep -q '"punch":true' && break
  sleep 0.25
done
if echo "$NL" | grep -q '"name":"NAT Host"' && echo "$NL" | grep -q '"nat":"cone"' && echo "$NL" | grep -q '"punch":true' \
   && echo "$NL" | grep -q "\"port\":$N_GAME" && nlog "$SCRATCH/n_sv.log" | grep -q "STUN: public endpoint of the game port" \
   && nlog "$SCRATCH/n_sv.log" | grep -q "punch relay connected"; then
  pass "N1 host: STUN from the game socket, listed nat=cone on its port, punch relay socket open"
else fail "N1 host STUN / listing / relay" "list=$NL | $(nlog "$SCRATCH/n_sv.log" | grep -iE 'stun|punch|master' | tail -4)"; fi

# N2: direct join only: every Hello is dropped by the emulated NAT
ND1="$SCRATCH/nat_direct"; mkdir -p "$ND1"
sc_launch "$ND1" --parent-pid "$E2E_WINPID" --server "127.0.0.1:$N_GAME" --state-dir "$ND1" --nick "Direct" \
  --punch off > "$SCRATCH/n_direct.log" 2>&1 &
N_D1=$!; N_PIDS+=($N_D1)
sleep 8
if ! link_is "$ND1" connected && nlog "$SCRATCH/n_sv.log" | grep -q "emulated NAT: unsolicited inbound datagram dropped"; then
  pass "N2 direct join behind the NAT fails (8 s, Hellos dropped: $(link_show "$ND1"))"
else fail "N2 direct join should fail" "$(link_show "$ND1") | $(nlog "$SCRATCH/n_sv.log" | grep -c dropped) drops"; fi
kill "$N_D1" 2>/dev/null; wait "$N_D1" 2>/dev/null

# N3: the same join with traversal: STUN, punch request, probes, handshake
ND2="$SCRATCH/nat_punch"; mkdir -p "$ND2"
N3_T0=$(date +%s)
sc_launch "$ND2" --parent-pid "$E2E_WINPID" --server "127.0.0.1:$N_GAME" --state-dir "$ND2" --nick "Punched" \
  --punch force --master "$NB" --stun "127.0.0.1:$N_STUN" --events "$ND2/events.jsonl" > "$SCRATCH/n_punch.log" 2>&1 &
N_D2=$!; N_PIDS+=($N_D2)
if wait_link 30 "$ND2" connected; then
  N3_S=$(( $(date +%s) - N3_T0 ))
  if grep -q '"step":"relayed"' "$ND2/events.jsonl" && grep -q '"nat_probe_rx"' "$ND2/events.jsonl" \
     && nlog "$SCRATCH/n_sv.log" | grep -q "punch: probing the joiner's public endpoint" \
     && nlog "$SCRATCH/n_master.log" | grep -q "punch request relayed"; then
    pass "N3 punched join connects in ${N3_S} s (STUN -> punch request -> host probes -> handshake; peer $(link_get "$ND2" my_peer_id))"
  else fail "N3 punched join: evidence missing" "$(grep -o '"step":"[a-z]*"' "$ND2/events.jsonl" | tr '\n' ' ') | $(nlog "$SCRATCH/n_sv.log" | grep -i punch | tail -2)"; fi
else
  fail "N3 punched join did not connect" "$(link_show "$ND2") | $(nlog "$SCRATCH/n_punch.log" | grep -iE 'punch|stun|traversal' | tail -5)"
fi
sleep 0.5
if [[ "$(link_get "$ND2" reason)" == "" ]] && nlog "$SCRATCH/n_punch.log" | grep -q "connected through NAT traversal"; then
  pass "N3b the reason line is cleared once connected"
else fail "N3b reason line" "$(link_show "$ND2")"; fi

# N4: probes go to the endpoint asked for, and only a few small ones
"$HSMP_TOOLS" natlab catch --bind "127.0.0.1:$N_CATCH" --secs 8 > "$SCRATCH/n_catch.log" 2>&1 &
N_CP=$!; N_PIDS+=($N_CP)
e2e_wait_bound "$N_CATCH" "$N_CP" 5
P1=$(curl -s -o /dev/null -w '%{http_code}' -X POST "$NB/v1/punch" -H 'content-type: application/json' \
     -d "{\"host\":\"127.0.0.1\",\"port\":$N_GAME,\"endpoint\":\"127.0.0.1:$N_CATCH\",\"nonce\":\"e2e\"}")
wait "$N_CP" 2>/dev/null
CATCH=$(tr -d '\r' < "$SCRATCH/n_catch.log")
if [[ "$P1" == "202" ]] && [[ "$CATCH" == "probes 4 from 127.0.0.1:$N_GAME" ]]; then
  pass "N4 relayed punch: 4 probes from the game port to exactly the endpoint asked for"
else fail "N4 probes" "status=$P1 catch=$CATCH"; fi

# N5: what the relay refuses
P_OTHER=$(curl -s -o /dev/null -w '%{http_code}' -X POST "$NB/v1/punch" -H 'content-type: application/json' \
     -d "{\"host\":\"127.0.0.1\",\"port\":$N_GAME,\"endpoint\":\"10.9.8.7:40000\"}")
P_LOW=$(curl -s -o /dev/null -w '%{http_code}' -X POST "$NB/v1/punch" -H 'content-type: application/json' \
     -d "{\"host\":\"127.0.0.1\",\"port\":$N_GAME,\"endpoint\":\"127.0.0.1:53\"}")
P_NONE=$(curl -s -o /dev/null -w '%{http_code}' -X POST "$NB/v1/punch" -H 'content-type: application/json' \
     -d "{\"host\":\"127.0.0.1\",\"port\":1999,\"endpoint\":\"127.0.0.1:40000\"}")
N429=0
for i in $(seq 1 12); do
  c=$(curl -s -o /dev/null -w '%{http_code}' -X POST "$NB/v1/punch" -H 'content-type: application/json' \
      -d "{\"host\":\"127.0.0.1\",\"port\":$N_GAME,\"endpoint\":\"127.0.0.1:$N_CATCH\"}")
  [[ "$c" == "429" ]] && N429=$((N429+1))
done
if [[ "$P_OTHER" == "403" && "$P_LOW" == "400" && "$P_NONE" == "404" && "$N429" -ge 5 ]]; then
  pass "N5 relay refuses a third-party endpoint (403), port < 1024 (400), unknown server (404); a flood is throttled ($N429/12 x 429)"
else fail "N5 relay checks" "other=$P_OTHER low=$P_LOW none=$P_NONE throttled=$N429/12"; fi

# N6: a host the list cannot punch (not listed): the player is told what to ask the host
e2e_port N_DEAD
ND3="$SCRATCH/nat_blocked"; mkdir -p "$ND3"
sc_launch "$ND3" --parent-pid "$E2E_WINPID" --server "127.0.0.1:$N_DEAD" --state-dir "$ND3" --nick "Blocked" \
  --punch force --master "$NB" --stun "127.0.0.1:$N_STUN" > "$SCRATCH/n_blocked.log" 2>&1 &
N_D3=$!; N_PIDS+=($N_D3)
blocked_shown() { [[ "$(link_get "$ND3" reason)" == "The host's network blocks incoming connections; ask them to forward UDP $N_DEAD" ]]; }
if rec_wait 30 blocked_shown "$ND3" && ! link_is "$ND3" connected; then
  pass "N6 no direct path and no relay: \"$(link_get "$ND3" reason)\""
else fail "N6 blocked message" "$(link_show "$ND3")"; fi
n_stop

if [[ -n "${E2E_NAT_STANDALONE:-}" ]]; then
  echo
  echo "SUMMARY: $PASS_COUNT passed, $FAIL_COUNT failed"
  (( FAIL_COUNT == 0 )) || { printf '  - %s\n' "${FAIL_NAMES[@]}"; exit 1; }
fi
