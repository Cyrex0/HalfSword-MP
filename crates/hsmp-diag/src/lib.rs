//! HSMP diagnostics on the player's machine.
//!
//! Every game run gets a session folder, `%LOCALAPPDATA%\HSMP\logs\<UTC stamp>_p<game pid>\`
//! (`HSMP_LOGS_DIR` overrides the root). At boot HSMPMenu runs `hsmp-sidecar --log-session
//! start`, which closes earlier sessions, creates the folder, prunes old ones and reports
//! whether the previous run crashed; then it starts `hsmp-sidecar --log-session watch`, a
//! small process that waits for the game to exit and copies UE4SS.log, the engine log, this
//! run's crash folders and the slices of the HSMP event logs into the folder. The sidecar and
//! a listen server write their own `sidecar.log` / `server.log` there (`--log-dir`).
//! Nothing here runs on the game thread.
//!
//! | module | job |
//! |---|---|
//! | `sessions` | folder names, `session.json`, listing, pruning (10 sessions, 200 MB) |
//! | `collect` | what the watcher copies when the game exits, and the run's outcome |
//! | `logfile` | the non-blocking, size-rotated log file behind `--log-dir` |
//! | `redact` | the Redactor: user names, home paths, e-mails, keys, tokens, nicks, IP addresses |
//! | `report` | the bug-report zip (manifest first, redacted text, size cap) of the launcher and `hsmp-server --report` |
//! | `proc` | wait for a process, its exit code and image path (Windows) |
//! | `triage` | the crash signature table and the CrashContext xml parser (`hsmp-tools crash-triage`, bug reports) |

pub mod collect;
pub mod logfile;
pub mod proc;
pub mod redact;
pub mod report;
pub mod sessions;
pub mod time;
pub mod triage;
