#!/usr/bin/env bash
# e2e-logs.sh - server diagnostics (sourced by scripts/e2e-test.sh; needs its helpers):
#   L1  a dedicated server with --log-dir writes server-<day>.log and server-events-<day>.jsonl:
#       the config line, the join, the 10 s `stats:` and `stats peer` lines, typed JSON events
#   L2  RCON REPORT prints the newest stats report
#   L3  the leave and the clean-shutdown summary are logged
#   L4  hsmp-server --report writes a redacted report zip of those logs
#   L5  a listen host (HSMP_LISTEN_HOST=1) writes server.log / server-events.jsonl into the
#       folder it is given (the game's session folder)

hdr "L1-L5: server log files, stats lines, RCON REPORT, --report"

e2e_port LPORT
e2e_port LRCON
LDIR="$SCRATCH/srv_logs"; mkdir -p "$LDIR"
LDIR_W="$(cygpath -m "$LDIR" 2>/dev/null || echo "$LDIR")"
"$BINS/hsmp-server.exe" --parent-pid "$E2E_WINPID" --bind "127.0.0.1:$LPORT" --tick-hz 30 --log-dir "$LDIR_W" \
  --rcon-bind "127.0.0.1:$LRCON" --rcon-password logpw > "$SCRATCH/l_sv.log" 2>&1 &
LSV=$!
e2e_wait_bound $LPORT $LSV
LSC_DIR="$SCRATCH/l_side"; mkdir -p "$LSC_DIR"
sc_launch "$LSC_DIR" --parent-pid "$E2E_WINPID" --server "127.0.0.1:$LPORT" --state-dir "$LSC_DIR" --nick "Logger" > "$SCRATCH/l_sc.log" 2>&1 &
LSC=$!
# two stats periods with the player connected
sleep 22
DAY=$(date -u +%Y%m%d)
LLOG="$LDIR/server-$DAY.log"
LEV="$LDIR/server-events-$DAY.jsonl"
if [[ -f "$LLOG" && -f "$LEV" ]] && grep -q "server config" "$LLOG" && grep -q "peer joined" "$LLOG" \
   && grep -q "stats: 1 player(s) | tick avg" "$LLOG" && grep -q 'stats peer 1 nick="Logger" 127.0.0.1:[0-9]*: rtt' "$LLOG" \
   && grep -q '"message":"peer joined"' "$LEV" && grep -q '"report":{"arena"' "$LEV" && grep -q '"pid":' "$LEV" \
   && ! grep -q $'\x1b\[' "$LLOG" && ! grep -q "logpw" "$LLOG" "$LEV"; then
  pass "L1 daily log + JSON events: config, join, stats lines, typed stats event, no colours, no RCON password"
else fail "L1 server log files" "$(ls -la "$LDIR" 2>&1 | tail -5) | $(grep -E 'stats|config' "$LLOG" 2>/dev/null | tail -3)"; fi

REP=$("$HSMP_TOOLS" rcon "127.0.0.1:$LRCON" "AUTH logpw@200" "REPORT@400" 2>/dev/null)
if echo "$REP" | grep -q "^stats: 1 player(s)" && echo "$REP" | grep -q "^stats peer 1" && echo "$REP" | grep -q "^END"; then
  pass "L2 RCON REPORT prints the newest stats report"
else fail "L2 RCON REPORT" "$REP"; fi

kill $LSC 2>/dev/null; wait $LSC 2>/dev/null
sleep 1
"$HSMP_TOOLS" rcon "127.0.0.1:$LRCON" "AUTH logpw@200" "SHUTDOWN@400" > /dev/null 2>&1
for i in $(seq 1 50); do kill -0 "$LSV" 2>/dev/null || break; sleep 0.2; done
if grep -q "shutdown summary" "$LLOG" && grep -q "joins=1" "$LLOG" && grep -q '"message":"shutdown summary"' "$LEV"; then
  pass "L3 the clean-shutdown summary is in both files"
else fail "L3 shutdown summary" "$(tail -3 "$LLOG")"; fi

LZIP="$SCRATCH/l_report.zip"
LOUT=$(HSMP_MASTER_URL="https://127.0.0.1:1" "$BINS/hsmp-server.exe" --report --log-dir "$LDIR_W" --report-out "$(cygpath -m "$LZIP" 2>/dev/null || echo "$LZIP")" 2>&1)
LRC=$?
if [[ $LRC -eq 0 && -s "$LZIP" ]] && echo "$LOUT" | grep -q "logs/server-$DAY.log .* redacted" && echo "$LOUT" | grep -q "system.json"; then
  pass "L4 hsmp-server --report writes the redacted report zip"
else fail "L4 --report" "rc=$LRC $LOUT"; fi

e2e_port LPORT2
LDIR2="$SCRATCH/srv_logs_listen"; mkdir -p "$LDIR2"
HSMP_LISTEN_HOST=1 "$BINS/hsmp-server.exe" --parent-pid "$E2E_WINPID" --bind "127.0.0.1:$LPORT2" --tick-hz 30 \
  --log-dir "$(cygpath -m "$LDIR2" 2>/dev/null || echo "$LDIR2")" > "$SCRATCH/l_sv2.log" 2>&1 &
LSV2=$!
e2e_wait_bound $LPORT2 $LSV2
sleep 1
kill $LSV2 2>/dev/null; wait $LSV2 2>/dev/null
if [[ -f "$LDIR2/server.log" && -f "$LDIR2/server-events.jsonl" ]] && grep -q "listen_host=true" "$LDIR2/server.log"; then
  pass "L5 a listen host logs server.log / server-events.jsonl into the session folder"
else fail "L5 listen-host log files" "$(ls -la "$LDIR2" 2>&1)"; fi
