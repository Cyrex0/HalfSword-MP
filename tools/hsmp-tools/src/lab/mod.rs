//! Evidence-based, incremental live combat experiments. Never certifies solo parity.
pub mod collect;
pub mod control;
pub mod stats;

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Recipe {
    pub name: String,
    pub arena: String,
    pub mode: String,
    pub kit_rules: String,
    pub kits: Vec<String>,
    pub ai: bool,
    pub probe: bool,
    #[serde(default)]
    pub drive: Option<String>,
    #[serde(default)]
    pub tune: BTreeMap<String, f64>,
    pub duration_s: u64,
    pub round_timeout_s: u64,
    #[serde(default)]
    pub no_claim_timeout_s: u64,
    #[serde(default)]
    pub commands: Vec<String>,
    pub bounds: BTreeMap<String, Bound>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Bound {
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub min_samples: usize,
}

impl Recipe {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.name.is_empty()
                && self
                    .name
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'-'),
            "invalid recipe name"
        );
        anyhow::ensure!(
            self.kits.len() == 2 && self.duration_s > 0 && self.round_timeout_s > 0,
            "two kits and positive durations required"
        );
        anyhow::ensure!(
            !self.probe || self.ai,
            "probe recipes require AI combat; visual recipes must disable probe"
        );
        anyhow::ensure!(
            !self.ai || self.drive.is_none(),
            "AI and scripted drive cannot own input together"
        );
        for s in [&self.arena, &self.mode, &self.kit_rules]
            .into_iter()
            .chain(self.commands.iter())
            .chain(self.kits.iter())
        {
            anyhow::ensure!(!s.contains(['\r', '\n']), "recipe contains a line break");
        }
        anyhow::ensure!(
            self.tune.values().all(|v| v.is_finite()),
            "nonfinite tune value"
        );
        for b in self.bounds.values() {
            anyhow::ensure!(
                b.min_samples > 0 && b.min.into_iter().chain(b.max).all(f64::is_finite),
                "invalid bound"
            );
            anyhow::ensure!(
                !matches!((b.min,b.max), (Some(a),Some(z)) if a>z),
                "reversed bound"
            );
        }
        Ok(())
    }
}
