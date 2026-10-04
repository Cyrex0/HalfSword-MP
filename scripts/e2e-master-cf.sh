#!/usr/bin/env bash
# e2e-master-cf.sh - the Cloudflare Worker server list (master-cf/) under `wrangler dev`,
# with the real hsmp-server registering against it and hsmp-query listing it:
#   CF1  health, empty list, no CORS, browser-origin writes refused
#   CF2  hsmp-server (HSMP_MASTER_URL) registers with a signed request; the listing has
#        the source address, the port, the server key
#   CF3  hsmp-query (the in-game browser's helper) lists the server and reaches it over UDP
#   CF4  signed heartbeats keep it listed past the TTL; unsigned and forged writes refused
#   CF5  RCON SHUTDOWN removes the listing (signed DELETE)
#   CF10 bug reports: size cap, non-report payloads refused, nothing public, burst limit
#   CF11 a report is stored (local R2) with an opaque id; the per-address daily cap
#   CF12 admin list / download only with REPORTS_ADMIN_TOKEN
#   CF13 admin delete and the retention sweep
#   CF6  punch relay: a host behind the emulated NAT holds its listen WebSocket on the Durable
#        Object; a punch request is relayed and the host probes the endpoint asked for
#   CF7  the relay refuses third-party endpoints, low ports, unknown servers, unsigned listens
#   CF8  a real sidecar joins that host through the Worker's punch relay (from e2e-test.sh)
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
  CF_ADMIN="e2e-admin-$$-$RANDOM$RANDOM"
  CF_LOG="$SCRATCH/cf_wrangler.log"
  # 5 s heartbeats and a 15 s TTL so expiry and keep-alive show within the test
  ( cd "$REPO/master-cf" && exec npx --yes wrangler@4.147.0 dev --ip 127.0.0.1 --port "$CF_HTTP" \
      --var HEARTBEAT_S:5 --var TTL_S:15 --var "REPORTS_ADMIN_TOKEN:$CF_ADMIN" --var REPORTS_PER_IP_DAY:1 --persist-to "$(cygpath -m "$SCRATCH/cf_state" 2>/dev/null || echo "$SCRATCH/cf_state")" ) \
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

    # CF10-CF13: bug-report uploads (master-cf/src/reports.rs) into the local R2 simulation
    RZ="$SCRATCH/cf_reports"; mkdir -p "$RZ"
    "$HSMP_TOOLS" report-fixture --out "$(cygpath -m "$RZ/ok.zip" 2>/dev/null || echo "$RZ/ok.zip")"
    "$HSMP_TOOLS" report-fixture --out "$(cygpath -m "$RZ/magic.zip" 2>/dev/null || echo "$RZ/magic.zip")" --magic nope
    "$HSMP_TOOLS" report-fixture --out "$(cygpath -m "$RZ/big.zip" 2>/dev/null || echo "$RZ/big.zip")" --pad-mb 26
    printf 'just some text, not a zip at all, but long enough to be checked\n' > "$RZ/text.txt"
    post_report() { curl -s -o "$RZ/last_body" -w '%{http_code}' -X POST "$B/v1/reports" -H 'content-type: application/zip' --data-binary "@$1"; }

    # CF10: not a report / too large / wrong methods, then the burst limit
    BIG=$(post_report "$RZ/big.zip")
    PUB=$(curl -s -o /dev/null -w '%{http_code}' "$B/v1/reports/20261004-0123456789abcdef0123456789abcdef")
    GETR=$(curl -s -o /dev/null -w '%{http_code}' "$B/v1/reports")
    BRW=$(curl -s -o /dev/null -w '%{http_code}' -X POST "$B/v1/reports" -H 'Origin: https://evil.example' -H 'content-type: application/zip' --data-binary "@$RZ/ok.zip")
    TXT=$(post_report "$RZ/text.txt"); TXT_B=$(cat "$RZ/last_body")
    MAG=$(post_report "$RZ/magic.zip"); MAG_B=$(cat "$RZ/last_body")
    # The limiter counts in fixed one-minute windows, so a burst that straddles a window edge
    # can get one extra request through. Keep posting refused payloads (they never use the
    # daily cap) until the limiter answers, instead of spending the one real upload on it.
    RATE=""; RATE_B=""
    for i in $(seq 1 12); do
      RATE=$(post_report "$RZ/magic.zip"); RATE_B=$(cat "$RZ/last_body")
      [[ "$RATE" == "429" ]] && break
    done
    if [[ "$BIG" == "413" && "$PUB" == "404" && "$GETR" == "405" && "$BRW" == "403" && "$TXT" == "400" && "$TXT_B" == "not a zip" \
          && "$MAG" == "400" && "$MAG_B" == *"magic"* && "$RATE" == "429" && "$RATE_B" == *"minute"* ]]; then
      pass "CF10 reports: 25 MB cap, non-report payloads refused, nothing served publicly, burst limit"
    else fail "CF10 reports rejects" "big=$BIG public=$PUB get=$GETR origin=$BRW text=$TXT($TXT_B) magic=$MAG($MAG_B) burst=$RATE($RATE_B)"; fi

    # CF11: after the burst window a report is stored and gets an opaque id; the address's
    # daily cap (REPORTS_PER_IP_DAY=1 here) refuses the next one
    sleep 61
    UP=$(post_report "$RZ/ok.zip"); UP_B=$(cat "$RZ/last_body")
    RID=$(echo "$UP_B" | grep -o '"id":"[0-9]\{8\}-[0-9a-f]\{32\}"' | cut -d'"' -f4)
    DAY=$(post_report "$RZ/ok.zip"); DAY_B=$(cat "$RZ/last_body")
    if [[ "$UP" == "201" && -n "$RID" && "$DAY" == "429" && "$DAY_B" == *"today"* ]]; then
      pass "CF11 a report is stored with an opaque id; the per-address daily cap holds"
    else fail "CF11 upload / daily cap" "upload=$UP($UP_B) second=$DAY($DAY_B)"; fi

    # CF12: admin list / download need the token; the download is the uploaded zip
    AUTH="authorization: Bearer $CF_ADMIN"
    NOAUTH=$(curl -s -o /dev/null -w '%{http_code}' "$B/v1/reports/admin")
    BADAUTH=$(curl -s -o /dev/null -w '%{http_code}' -H 'authorization: Bearer wrong-token-wrong-token' "$B/v1/reports/admin")
    LIST=$(curl -s -H "$AUTH" "$B/v1/reports/admin")
    curl -s -H "$AUTH" -o "$RZ/down.zip" "$B/v1/reports/admin/$RID"
    if [[ "$NOAUTH" == "403" && "$BADAUTH" == "403" ]] && echo "$LIST" | grep -q "\"id\":\"$RID\"" && ! echo "$LIST" | grep -q '"ip"' \
       && cmp -s "$RZ/ok.zip" "$RZ/down.zip"; then
      pass "CF12 admin list / download with the token only (no address in the listing)"
    else fail "CF12 admin" "noauth=$NOAUTH badauth=$BADAUTH list=$LIST"; fi

    # CF13: admin delete; the report is gone; sweep answers
    DEL=$(curl -s -o /dev/null -w '%{http_code}' -X DELETE -H "$AUTH" "$B/v1/reports/admin/$RID")
    GONE=$(curl -s -o /dev/null -w '%{http_code}' -H "$AUTH" "$B/v1/reports/admin/$RID")
    SW=$(curl -s -X POST -H "$AUTH" "$B/v1/reports/admin/sweep")
    if [[ "$DEL" == "204" && "$GONE" == "404" ]] && echo "$SW" | grep -q '"deleted":'; then
      pass "CF13 admin delete and the retention sweep"
    else fail "CF13 delete / sweep" "delete=$DEL after=$GONE sweep=$SW"; fi
    # CF6: the punch relay. A host behind the emulated NAT (--emulate-nat, a local STUN
    # server) opens its listen WebSocket to the Durable Object; a punch request is relayed
    # down it and the host probes exactly the endpoint asked for.
    e2e_port CF_GAME2; e2e_port CF_STUN; e2e_port CF_CATCH
    "$HSMP_TOOLS" natlab stun --bind "127.0.0.1:$CF_STUN" --secs 240 > "$SCRATCH/cf_stun.log" 2>&1 &
    CF_PIDS+=($!)
    HSMP_MASTER_URL="$B" HSMP_STUN_SERVERS="127.0.0.1:$CF_STUN" "$BINS/hsmp-server.exe" --parent-pid "$E2E_WINPID" \
      --bind "127.0.0.1:$CF_GAME2" --tick-hz 30 --name "CF NAT Host" --port-map off --emulate-nat > "$SCRATCH/cf_sv2.log" 2>&1 &
    CF_SV2=$!; CF_PIDS+=("$CF_SV2")
    L4=""
    for i in $(seq 1 60); do L4=$(curl -s "$B/v1/servers"); echo "$L4" | grep -q '"punch":true' && break; sleep 0.5; done
    CF_SID2=$(echo "$L4" | grep -o '"server_id":"[0-9a-f]*"' | head -1 | cut -d'"' -f4)
    "$HSMP_TOOLS" natlab catch --bind "127.0.0.1:$CF_CATCH" --secs 10 > "$SCRATCH/cf_catch.log" 2>&1 &
    CF_CP=$!; CF_PIDS+=("$CF_CP")
    sleep 0.5
    cf_punch() { curl -s -o /dev/null -w '%{http_code}' -X POST "$B/v1/punch" -H 'content-type: application/json' -d "$1"; }
    PR=$(cf_punch "{\"host\":\"127.0.0.1\",\"port\":$CF_GAME2,\"endpoint\":\"127.0.0.1:$CF_CATCH\",\"nonce\":\"cf\"}")
    wait "$CF_CP" 2>/dev/null
    CATCH=$(tr -d '\r' < "$SCRATCH/cf_catch.log")
    if echo "$L4" | grep -q '"nat":"cone"' && [[ "$PR" == "202" && "$CATCH" == "probes 4 from 127.0.0.1:$CF_GAME2" ]]; then
      pass "CF6 listen WebSocket on the Durable Object; a punch is relayed and the host sends 4 probes to the endpoint asked for"
    else fail "CF6 punch relay" "punch=$PR catch=$CATCH list=$L4 | $(sed 's/\x1b\[[0-9;]*m//g' "$SCRATCH/cf_sv2.log" | grep -iE 'punch|stun' | tail -3)"; fi

    # CF7: what the Worker refuses: a third-party endpoint, a low port, an unknown server, a
    # listen socket without a valid signature or without the upgrade
    P_OTHER=$(cf_punch "{\"host\":\"127.0.0.1\",\"port\":$CF_GAME2,\"endpoint\":\"10.9.8.7:40000\"}")
    P_LOW=$(cf_punch "{\"host\":\"127.0.0.1\",\"port\":$CF_GAME2,\"endpoint\":\"127.0.0.1:53\"}")
    P_NONE=$(cf_punch "{\"host\":\"127.0.0.1\",\"port\":1999,\"endpoint\":\"127.0.0.1:40000\"}")
    LP="/v1/punch/listen/$CF_SID2?ts=$(( $(date +%s) * 1000 ))"
    L_PLAIN=$(curl -s -o /dev/null -w '%{http_code}' "$B$LP")
    L_FORGED=$(curl -s -o /dev/null -w '%{http_code}' -m 5 "$B$LP" -H 'Connection: Upgrade' -H 'Upgrade: websocket' \
      -H 'Sec-WebSocket-Version: 13' -H 'Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==' -H "x-hsmp-sig: $(printf '0%.0s' $(seq 1 128))")
    if [[ "$P_OTHER" == "403" && "$P_LOW" == "400" && "$P_NONE" == "404" && "$L_PLAIN" == "426" && "$L_FORGED" == "403" ]]; then
      pass "CF7 Worker refuses third-party endpoints (403), port < 1024 (400), unknown servers (404), plain (426) and forged (403) listen requests"
    else fail "CF7 relay checks" "other=$P_OTHER low=$P_LOW none=$P_NONE listen-plain=$L_PLAIN listen-forged=$L_FORGED"; fi

    # CF8: a real join through the Worker: the direct path is filtered, the sidecar punches
    if declare -f sc_launch >/dev/null 2>&1 && declare -f wait_link >/dev/null 2>&1; then
      CFD="$SCRATCH/cf_punch_sc"; mkdir -p "$CFD"
      sc_launch "$CFD" --parent-pid "$E2E_WINPID" --server "127.0.0.1:$CF_GAME2" --state-dir "$CFD" --nick "CFPunch" \
        --punch force --master "$B" --stun "127.0.0.1:$CF_STUN" > "$SCRATCH/cf_punch_sc.log" 2>&1 &
      CF_PIDS+=($!)
      if wait_link 30 "$CFD" connected && sed 's/\x1b\[[0-9;]*m//g' "$SCRATCH/cf_punch_sc.log" | grep -q "punch request relayed"; then
        pass "CF8 a joiner behind no forwarded port connects through the Worker's punch relay"
      else fail "CF8 punched join via the Worker" "$(link_show "$CFD") | $(sed 's/\x1b\[[0-9;]*m//g' "$SCRATCH/cf_punch_sc.log" | grep -iE 'punch|stun|traversal' | tail -4)"; fi
    else
      cf_skip "CF8 needs e2e-test.sh's sc_launch (run from scripts/e2e-test.sh)"
    fi
    cf_stop
  fi
fi

if [[ -n "${E2E_CF_STANDALONE:-}" ]]; then
  echo
  echo "SUMMARY: $PASS_COUNT passed, $FAIL_COUNT failed"
  (( FAIL_COUNT == 0 )) || { printf '  - %s\n' "${FAIL_NAMES[@]}"; exit 1; }
fi
