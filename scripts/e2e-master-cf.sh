#!/usr/bin/env bash
# e2e-master-cf.sh - the Cloudflare Worker server list (master-cf/) under `wrangler dev`,
# with the real hsmp-server registering against it and hsmp-query listing it:
#   CF1  health, empty list, no CORS, browser-origin writes refused
#   CF2  hsmp-server (HSMP_MASTER_URL) registers with a signed request; the listing has
#        the source address, the port, the server key
#   CF3  hsmp-query (the in-game browser's helper) lists the server and reaches it over UDP
#   CF4  signed heartbeats keep it listed past the TTL; unsigned and forged writes refused
#   CF5  RCON SHUTDOWN removes the listing (signed DELETE)
#
# Skipped (not failed) without Node.js >= 22 + npx (wrangler 4 needs it; HSMP_NODE_DIR may
# point at a Node folder that is not on PATH), worker-build, or the wasm32 target.
#
# Sourced by scripts/e2e-test.sh, or on its own:
#   HSMP_TOOLS=<target>/release/hsmp-tools.exe HSMP_BINS=<target>/release bash scripts/e2e-master-cf.sh

if ! declare -f pass >/dev/null 2>&1; then
  set -u -o pipefail
  e2e_unix_path() { if command -v cygpath >/dev/null 2>&1; then cygpath -u "$1"; else printf '%s' "$1"; fi; }
  REPO="${HSMP_REPO:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
  E2E_TARGET="${CARGO_TARGET_DIR:+$(e2e_unix_path "$CARGO_TARGET_DIR")}"
  BINS="${HSMP_BINS:-${E2E_TARGET:-$REPO/target}/release}"
  HSMP_TOOLS="${HSMP_TOOLS:-${E2E_TARGET:-$REPO/target}/release/hsmp-tools.exe}"
  if [[ ! -x "$HSMP_TOOLS" ]]; then echo "hsmp-tools not found at $HSMP_TOOLS (set HSMP_TOOLS to the .exe)" >&2; exit 1; fi
  E2E_WINPID=$(cat /proc/$$/winpid 2>/dev/null || echo $$)
  SCRATCH="${HSMP_SCRATCH:-${TMPDIR:-/tmp}/hsmp-e2e-cf-$$-$(date +%s)}"
  mkdir -p "$SCRATCH"
  export HSMP_STATE_DIR="$SCRATCH/home"; mkdir -p "$HSMP_STATE_DIR"
  PASS_COUNT=0; FAIL_COUNT=0; FAIL_NAMES=()
  pass() { PASS_COUNT=$((PASS_COUNT+1)); printf '  \e[32mPASS\e[0m  %s\n' "$1"; }
  fail() { FAIL_COUNT=$((FAIL_COUNT+1)); FAIL_NAMES+=("$1"); printf '  \e[31mFAIL\e[0m  %s\n' "$1" >&2; printf '        %s\n' "${2:-}" >&2; }
  hdr()  { printf '\n\e[36m== %s ==\e[0m\n' "$1"; }
  E2E_CF_STANDALONE=1
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
fi

hdr "CF: Cloudflare Worker server list (master-cf, wrangler dev)"

cf_skip() { printf '  \e[33mSKIP\e[0m  %s\n' "CF: $1"; }
[[ -n "${HSMP_NODE_DIR:-}" ]] && export PATH="$(e2e_unix_path "$HSMP_NODE_DIR"):$PATH"
export PATH="$HOME/.cargo/bin:$PATH"
CF_NODE_MAJOR=$(node --version 2>/dev/null | sed -E 's/^v([0-9]+).*/\1/')
if ! command -v npx >/dev/null 2>&1 || [[ -z "$CF_NODE_MAJOR" ]]; then
  cf_skip "node/npx not found (install Node.js 22+ to run the Worker locally)"
elif (( CF_NODE_MAJOR < 22 )); then
  cf_skip "Node.js $(node --version) is too old for wrangler 4 (needs 22+; set HSMP_NODE_DIR)"
elif ! command -v worker-build >/dev/null 2>&1; then
  cf_skip "worker-build not installed (cargo install worker-build --locked)"
elif ! rustup target list --installed 2>/dev/null | grep -q wasm32-unknown-unknown; then
  cf_skip "wasm32-unknown-unknown target missing (rustup target add wasm32-unknown-unknown)"
else
  CF_PIDS=()
  e2e_port CF_HTTP
  e2e_port CF_GAME
  e2e_port CF_RCON
  B="http://127.0.0.1:$CF_HTTP"
  CF_LOG="$SCRATCH/cf_wrangler.log"
  # 5 s heartbeats and a 15 s TTL so expiry and keep-alive show within the test
  ( cd "$REPO/master-cf" && exec npx --yes wrangler@4.147.0 dev --ip 127.0.0.1 --port "$CF_HTTP" \
      --var HEARTBEAT_S:5 --var TTL_S:15 --persist-to "$(cygpath -m "$SCRATCH/cf_state" 2>/dev/null || echo "$SCRATCH/cf_state")" ) \
      > "$CF_LOG" 2>&1 &
  CF_WR=$!
  CF_WR_WIN=$(cat /proc/$CF_WR/winpid 2>/dev/null || echo "")
  cf_stop() {
    local p
    for p in "${CF_PIDS[@]}"; do kill "$p" 2>/dev/null; done
    # npx starts wrangler through a shim that exits, so the node processes are not children
    # of this shell: find this run's ones by their unique --port and stop each tree by PID.
    local w
    for w in $(powershell -NoProfile -Command "Get-CimInstance Win32_Process -Filter \"Name='node.exe'\" | Where-Object { \$_.CommandLine -match 'wrangler.* --port $CF_HTTP( |\$)' } | ForEach-Object { \$_.ProcessId }" 2>/dev/null | tr -d '\r'); do
      taskkill //PID "$w" //T //F >/dev/null 2>&1
    done
    [[ -n "$CF_WR_WIN" ]] && taskkill //PID "$CF_WR_WIN" //T //F >/dev/null 2>&1
    kill "$CF_WR" 2>/dev/null
    wait 2>/dev/null
  }
  CF_UP=no
  for i in $(seq 1 240); do   # first run compiles the Worker
    [[ "$(curl -s -m 2 "$B/v1/health")" == "ok" ]] && { CF_UP=yes; break; }
    kill -0 "$CF_WR" 2>/dev/null || break
    sleep 1
  done
  if [[ "$CF_UP" != yes ]]; then
    fail "CF1 wrangler dev did not come up" "$(tail -20 "$CF_LOG")"
    cf_stop
  else
    # CF1
    L0=$(curl -s "$B/v1/servers")
    ACAO=$(curl -s -i -H 'Origin: https://evil.example' "$B/v1/servers" | grep -ci '^access-control-allow-origin')
    OW=$(curl -s -o /dev/null -w '%{http_code}' -X POST "$B/v1/register" -H 'Origin: https://evil.example' -H 'content-type: application/json' -d '{}')
    if [[ "$L0" == "[]" && "$ACAO" == "0" && "$OW" == "403" ]]; then pass "CF1 health, empty list, no CORS, browser writes refused"
    else fail "CF1 basics" "list=$L0 acao=$ACAO origin-write=$OW"; fi

    # CF2
    HSMP_MASTER_URL="$B" "$BINS/hsmp-server.exe" --parent-pid "$E2E_WINPID" --bind "127.0.0.1:$CF_GAME" --tick-hz 30 \
      --name "CF Server" --mode "Best of 3" --region "EU" \
      --rcon-bind "127.0.0.1:$CF_RCON" --rcon-password cfpw > "$SCRATCH/cf_sv.log" 2>&1 &
    CF_SV=$!; CF_PIDS+=("$CF_SV")
    L1=""
    for i in $(seq 1 30); do L1=$(curl -s "$B/v1/servers"); echo "$L1" | grep -q '"CF Server"' && break; sleep 0.5; done
    if echo "$L1" | grep -q '"name":"CF Server"' && echo "$L1" | grep -q '"host":"127.0.0.1"' \
       && echo "$L1" | grep -q "\"port\":$CF_GAME" && echo "$L1" | grep -Eq '"server_key":"[0-9a-f]{64}"' \
       && sed 's/\x1b\[[0-9;]*m//g' "$SCRATCH/cf_sv.log" | grep -q "heartbeat_s=5"; then
      pass "CF2 hsmp-server registered (signed), listed at its source address with its key"
    else fail "CF2 register" "list=$L1 | $(grep -i master "$SCRATCH/cf_sv.log" | tail -3)"; fi
    CF_SID=$(echo "$L1" | grep -o '"server_id":"[0-9a-f]*"' | head -1 | cut -d'"' -f4)

    # CF3
    Q=$("$BINS/hsmp-query.exe" --master "$B" --gen cf 2>/dev/null)
    if echo "$Q" | grep -q $'^status\tok' \
       && echo "$Q" | awk -F'\t' -v p="$CF_GAME" '$1=="S" && $2=="127.0.0.1" && $3==p && $4=="CF Server" && $14=="master" && $15=="1" {f=1} END {exit !f}'; then
      pass "CF3 hsmp-query lists it from the Worker and the server answers its UDP query"
    else fail "CF3 hsmp-query" "$Q"; fi

    # CF4: alive after 2x the TTL only through heartbeats
    sleep 32
    L2=$(curl -s "$B/v1/servers")
    AGE=$(echo "$L2" | grep -o '"age_s":[0-9]*' | head -1 | cut -d: -f2)
    HB=$(grep -c "POST /v1/heartbeat/.* 204" "$CF_LOG")
    UN=$(curl -s -o /dev/null -w '%{http_code}' -X POST "$B/v1/register" -H 'content-type: application/json' \
         -d "{\"name\":\"Legacy\",\"port\":$CF_GAME,\"nonce\":\"n\",\"hmac\":\"\"}")
    FH=$(curl -s -o /dev/null -w '%{http_code}' -X POST "$B/v1/heartbeat/$CF_SID" -H 'content-type: application/json' \
         -H "x-hsmp-sig: $(printf '0%.0s' $(seq 1 128))" -d "{\"players\":0,\"ts\":$(( $(date +%s) * 1000 + 999999 ))}")
    FD=$(curl -s -o /dev/null -w '%{http_code}' -X DELETE "$B/v1/servers/$CF_SID" -H 'content-type: application/json' -d '{"nonce":"d","hmac":"x"}')
    if echo "$L2" | grep -q '"CF Server"' && [[ -n "$AGE" && "$AGE" -lt 15 && "$HB" -ge 3 && "$UN" == "400" && "$FH" == "403" && "$FD" == "403" ]]; then
      pass "CF4 signed heartbeats keep it listed; unsigned register / forged heartbeat / unsigned delete refused"
    else fail "CF4 heartbeat / auth" "age=$AGE heartbeats=$HB unsigned=$UN forged=$FH delete=$FD list=$L2"; fi

    # CF5
    "$HSMP_TOOLS" rcon "127.0.0.1:$CF_RCON" "AUTH cfpw@200" "SHUTDOWN@400" > /dev/null 2>&1
    for i in $(seq 1 30); do kill -0 "$CF_SV" 2>/dev/null || break; sleep 0.2; done
    for i in $(seq 1 20); do L3=$(curl -s "$B/v1/servers"); [[ "$L3" == "[]" ]] && break; sleep 0.5; done
    if [[ "$L3" == "[]" ]] && grep -q "removed from master" "$SCRATCH/cf_sv.log"; then
      pass "CF5 a clean shutdown removes the listing (signed DELETE)"
    else fail "CF5 deregister" "list=$L3 | $(tail -3 "$SCRATCH/cf_sv.log")"; fi
    cf_stop
  fi
fi

if [[ -n "${E2E_CF_STANDALONE:-}" ]]; then
  echo
  echo "SUMMARY: $PASS_COUNT passed, $FAIL_COUNT failed"
  (( FAIL_COUNT == 0 )) || { printf '  - %s\n' "${FAIL_NAMES[@]}"; exit 1; }
fi
