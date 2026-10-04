# Bug reports and diagnostics: how it works, how to fetch reports

Players send logs through the launcher's **Create bug report** (or `hsmp-launcher report`);
server hosts through `hsmp-server --report`. Both make the same zip, which goes to a GitHub
issue or to the Worker's `/v1/reports` (R2). This page is for us: the pieces, the formats, the
Cloudflare setup, and how to list and download reports.

## The pieces

| Piece | Where | Job |
|---|---|---|
| Session folders | `crates/hsmp-diag/src/sessions.rs` | `%LOCALAPPDATA%\HSMP\logs\<UTC stamp>_p<game pid>\` + `session.json`; newest 10 kept, 200 MB in all (`HSMP_LOGS_DIR` overrides the root) |
| Session start | `hsmp-sidecar --log-session start --parent-pid <game>` (`server/src/sidecar/session_log.rs`) | Run by HSMPMenu at boot (`mods/HSMPMenu/Scripts/diag.lua`, captured and polled from the 500 ms game-thread loop): closes runs whose watcher died, creates the folder, prunes, prints `dir=`, `id=`, `prev_id=`, `prev_outcome=`, `prev_crash=` |
| Session watcher | `hsmp-sidecar --log-session watch --session-dir D --parent-pid P --state-dir S` | Spawned by the menu (not a role: never killed by CANCEL). Waits on the game's process handle, gives the crash reporter up to 20 s, then `collect::finish` copies UE4SS.log, `Saved\Logs\HalfswordUE5.log`, this run's `UECC-*` folders and UE4SS dumps, and the run's lines of `hsmp_events*.jsonl` / `.career_guard.jsonl`; sets the outcome (`clean`, `crashed`, `abnormal`, `unfinished`) |
| Rescue | `ops::rescue_session_logs` (launcher start-up check, before Launch) | A run whose watcher never finished (PC off): its UE4SS.log is copied before the next start overwrites it |
| Crash note | `MX.crash_note_sync` in HSMPMenu | "The game crashed last time. Open the launcher > Create bug report ..." under the ribbons when `prev_outcome=crashed` |
| Process logs | `server/src/log_init.rs`, `crates/hsmp-diag/src/logfile.rs` | `--log-dir`: the sidecar's `sidecar.log`, a listen server's `server.log` + `server-events.jsonl` (size-rotated in the session folder); a dedicated server's daily `server-<day>.log` + `server-events-<day>.jsonl` (14 days, 500 MB). Background writer thread, whole lines, never blocks |
| Stats | `server/src/stats.rs`, the sidecar's 5 s report (`sidecar/pose.rs`) | Server: `stats:` / `stats peer` every 10 s + a typed `stats event` in the events file; sidecar: `link:` (RTT, loss), per-peer pose rates, latency |
| Redaction | `crates/hsmp-diag/src/redact.rs` (`Redactor`) | User names / home paths, computer name, e-mails, Steam ids, secret-looking keys and flags, the install's player key, nicks; IPs per `IpMode` (`Public`: `<ip-N>` numbering, LAN kept; `OwnOnly`; `All`) and the player's own public address (`/v1/myaddr`) always |
| Report zip | `crates/hsmp-diag/src/report.rs` | Entries exactly as sent; `hsmp-report.json` first (stored), 25 MB cap (dumps dropped first, then text cut from the start) |
| Launcher report | `launcher/src/report.rs`, `sysinfo.rs`, the GUI's Bug report section | `system.json` (Windows, CPU, RAM, GPUs, HSMP build identity via `hsmp-sidecar --build-info`, game build, UE4SS, firewall), `triage.json` (`hsmp_diag::triage::quick` per crash folder), the sessions, `launcher.log` |
| Server report | `server/src/server_report.rs` (`hsmp-server --report`) | `system.json` + the last N days of the log folder; kind `server` |
| Upload endpoint | `master-cf/src/reports.rs` | below |
| Format check | `crates/hsmp-master-core/src/reports.rs` | Shared by the Worker and the tests: first entry `hsmp-report.json`, stored, sizes in the local header, `{"magic":"hsmp-report","format":1}`, a valid end-of-central-directory |

## The zip

```text
hsmp-report.json          {"magic","format","kind":"client"|"server","launcher","hsmp","sessions",
                           "options","files":[{"name","size","sha256","redacted"}],"left_out"}
system.json
triage.json               (launcher)
sessions/<run id>/...     (launcher: every file of the run's folder; *.dmp unchanged)
launcher.log              (launcher: the last 3000 lines)
logs/server-<day>.log ... (server)
```

`triage.json` gives each run's outcome and exit code (`0xC0000005` etc.) and, per crash folder,
the signature our table matches without a debugger. Symbolise a dump from the zip as usual:
`hsmp-tools crash-triage --dir <unzipped>/sessions/<run>/crashes`.

## The upload endpoint

| Route | Who | Does |
|---|---|---|
| `POST /v1/reports` (body: the zip, `content-type: application/zip`) | launcher, `hsmp-server --report-upload` | `201 {"id":"<YYYYMMDD>-<32 hex>","retention_days":60}`; `400 <reason>` for anything that is not a report; `411` without a length; `413` above 25 MB; `429` from the burst limit (`REPORT_RATE`: 2 a minute per address, plus a per-isolate bucket), the per-address day cap (`REPORTS_PER_IP_DAY`, 3) or the day's total (`REPORTS_DAY_BYTES`, 150 MB); `503` without the R2 binding |
| `GET /v1/reports/admin[?day=YYYYMMDD][&cursor=C]` | us, `authorization: Bearer <REPORTS_ADMIN_TOKEN>` | `{"reports":[{"id","size","uploaded_ms","meta":{"kind","hsmp","launcher","sessions","entries"}}],"cursor"}` |
| `GET /v1/reports/admin/<id>` | us | the zip |
| `DELETE /v1/reports/admin/<id>` | us | `204` |
| `POST /v1/reports/admin/sweep` | us | deletes days older than `REPORTS_RETENTION_DAYS` (the bucket's lifecycle rule does this anyway) |

Without the `REPORTS_ADMIN_TOKEN` secret (16 characters or more) the admin routes answer 404.
Nothing under `/v1/reports` is ever served without it. An object is `r/<day>/<hex>.zip` with
custom metadata `ip` (a per-day SHA-256 tag of the uploader's address or IPv6 /64, used only to
count uploads; the admin listing leaves it out), `kind`, `hsmp`, `launcher`, `sessions`,
`entries`. The 150 MB a day cap keeps 60 days under R2's free 10 GB-month (free tier also has
1 M Class A and 10 M Class B operations a month; an upload costs one list and one put).

## Cloudflare setup (once)

From `master-cf/`, with the account's API token (`%LOCALAPPDATA%\HSMP\cloudflare\deploy.env`) and
Node 22:

```powershell
npx wrangler@4.147.0 r2 bucket create hsmp-reports
npx wrangler@4.147.0 r2 bucket lifecycle add hsmp-reports expire-reports r/ --expire-days 60 --force
npx wrangler@4.147.0 secret put REPORTS_ADMIN_TOKEN        # paste a long random string, keep it
npx wrangler@4.147.0 deploy
```

`wrangler.toml` already has the `REPORTS` R2 binding, the `REPORT_RATE` rate limiter
(`namespace_id = "2001"`) and the `REPORTS_*` vars. The bucket must exist before the first
deploy with this binding (the deploy fails otherwise). Check it:

```bash
curl -s -o /dev/null -w '%{http_code}\n' -X POST https://master.halfswordmp.workers.dev/v1/reports \
     -H 'content-type: application/zip' --data-binary 'not a zip'        # 400
```

## Fetching reports

With the token in `$T`:

```bash
M=https://master.halfswordmp.workers.dev
curl -s -H "authorization: Bearer $T" "$M/v1/reports/admin?day=20261004" | jq .
curl -s -H "authorization: Bearer $T" -o r.zip "$M/v1/reports/admin/20261004-0123456789abcdef0123456789abcdef"
curl -s -X DELETE -H "authorization: Bearer $T" "$M/v1/reports/admin/<id>"
```

Or straight from R2 with wrangler (no token needed, the account's own credentials):

```bash
npx wrangler@4.147.0 r2 object get hsmp-reports/r/20261004/0123456789abcdef0123456789abcdef.zip --remote --file r.zip
```

A player who quotes an id `20261004-0123...` maps to the key `r/20261004/0123....zip`.

## Tests

| Where | What |
|---|---|
| `cargo test -p hsmp-diag` | session naming, listing, pruning by count and size (current and live sessions kept), stale-session closing, the watcher's collection of a crashed run, the rescue, the size- and day-rotated log files, the report zip against the upload check |
| `cargo test -p hsmp-launcher` (crash.rs, report.rs) | redaction (paths in every slash style, keys, flags, Debug fields, tokens, e-mails, Steam ids, IPv4 / IPv6 numbering, LAN kept, own IP), the launcher zip and manifest, the cap, the GitHub URL |
| `cargo test -p hsmp-server` | `stats` line formatting and counters, `log_init` filters and JSON lines, `server_report`; `tests/log_session.rs` runs the real sidecar's `start` / `watch` / next start and `--log-dir` |
| `hsmp-tools lua-test diag` | diag.lua: start captured and polled, the watcher's arguments, `--log-dir`, failures |
| `scripts/e2e-master-cf.sh` CF10-CF13 | the Worker under `wrangler dev` with local R2: size cap, non-report payloads, no public reads, burst limit, a stored report and its id, the day cap, admin list / download / delete / sweep |
| `scripts/e2e-logs.sh` L1-L5 | a real server with `--log-dir`: the daily files, config / join / stats / stats peer lines, typed events, RCON `REPORT`, the shutdown summary, `--report`, a listen host's `server.log` |
