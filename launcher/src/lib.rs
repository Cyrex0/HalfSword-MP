//! hsmp-launcher library: everything the launcher does, minus the window.
//!
//! | module | job |
//! |---|---|
//! | `steam`, `vdf` | find Steam and the Half Sword install (libraryfolders.vdf, appmanifest) |
//! | `game` | game layout, exe hash vs supported builds, conflicting UE4SS layouts |
//! | `manifest`, `sign`, `package`, `pack` | signed release manifest: schema, ed25519, verification, writing |
//! | `trust` | which keys count (compiled in / pinned outside the package), the launcher copy for updates |
//! | `install` | journaled install / update / uninstall / status, Engine.ini merge |
//! | `modstxt`, `cfgfile`, `ini` | mods.txt, hsmp.cfg and ini merging |
//! | `saves` | career save backup / restore (never deletes a backup; lists the career guard's MP backups too) |
//! | `careerguard` | recovers career-guard sessions a crash left open, at launcher start, after install, before Launch through Steam and before Uninstall (sidecar `--career-recover`) |
//! | `launch`, `procs` | start the game through Steam; read-only process checks |
//! | `update` | update check, download and verified install from the GitHub releases |
//! | `crash` | crash-report consent and redaction (interface only, no network) |
//! | `firewall` | the Windows Firewall allow rule for the installed hsmp-server.exe (added through UAC) |
//! | `ops` | high-level operations shared by the CLI and the GUI |

pub mod careerguard;
pub mod cfgfile;
pub mod crash;
pub mod firewall;
pub mod game;
pub mod ini;
pub mod install;
pub mod launch;
pub mod manifest;
pub mod modstxt;
pub mod ops;
pub mod pack;
pub mod package;
pub mod procs;
pub mod saves;
pub mod sign;
pub mod steam;
pub mod trust;
pub mod update;
pub mod util;
pub mod vdf;

#[cfg(test)]
pub(crate) mod testutil;

#[cfg(test)]
mod package_tests;

/// This launcher's version (Cargo package version).
pub const LAUNCHER_VERSION: &str = env!("CARGO_PKG_VERSION");
