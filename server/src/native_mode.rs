//! Bootstrap intent for the native game, not an alternative combat rules engine.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum Mode {
    #[default]
    Pvp,
    /// Explicit development scaffold: two native players and one native foe.
    Diagnostic,
}
impl Mode {
    pub fn parse(value: &str) -> Result<Self, &'static str> {
        match value { "pvp" => Ok(Self::Pvp), "diagnostic" => Ok(Self::Diagnostic), _ => Err("unsupported native mode") }
    }
    pub fn as_str(self) -> &'static str { match self { Self::Pvp => "pvp", Self::Diagnostic => "diagnostic" } }
    pub fn initial_entities(self) -> u32 { match self { Self::Pvp => 2, Self::Diagnostic => 3 } }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pvp_has_only_two_players_and_scaffold_requires_explicit_selection() {
        assert_eq!(Mode::default(), Mode::Pvp);
        assert_eq!(Mode::default().initial_entities(), 2);
        assert_eq!(Mode::parse("diagnostic").unwrap().initial_entities(), 3);
        for unsupported in ["", "abyss", "coop", "PVP"] { assert!(Mode::parse(unsupported).is_err()); }
    }
}
