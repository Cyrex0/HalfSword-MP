//! Launcher renderer policy; independent of the game's graphics settings.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    Dx12,
    Glow,
    Wgpu,
}

impl Backend {
    pub fn from_override(value: Option<&str>) -> Result<Self, String> {
        match value.map(str::trim).filter(|v| !v.is_empty()) {
            None => Ok(Self::Glow),
            Some(v) if v.eq_ignore_ascii_case("dx12") && cfg!(windows) => Ok(Self::Dx12),
            Some(v) if v.eq_ignore_ascii_case("glow") => Ok(Self::Glow),
            Some(v) if v.eq_ignore_ascii_case("wgpu") => Ok(Self::Wgpu),
            Some(v) => Err(format!("unknown or unavailable HSMP_LAUNCHER_RENDERER '{v}'; use glow, wgpu, or dx12 (Windows only)")),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Dx12 => "Direct3D 12",
            Self::Glow => "OpenGL",
            Self::Wgpu => "wgpu (automatic backend)",
        }
    }

    pub fn fallback(self, explicit: bool, app_created: bool) -> Option<Self> {
        (cfg!(windows) && self == Self::Glow && !explicit && !app_created).then_some(Self::Dx12)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portable_default_and_explicit_overrides() {
        let expected = Backend::Glow;
        assert_eq!(Backend::from_override(None).unwrap(), expected);
        assert_eq!(Backend::from_override(Some(" ")).unwrap(), expected);
        assert_eq!(Backend::from_override(Some(" GLOW ")).unwrap(), Backend::Glow);
        assert_eq!(Backend::from_override(Some("wgpu")).unwrap(), Backend::Wgpu);
        assert!(Backend::from_override(Some("typo")).unwrap_err().contains("HSMP_LAUNCHER_RENDERER"));
        assert_eq!(Backend::from_override(Some("dx12")).is_ok(), cfg!(windows));
    }

    #[test]
    fn fallback_only_after_default_opengl_startup_failure() {
        let expected = cfg!(windows).then_some(Backend::Dx12);
        assert_eq!(Backend::Glow.fallback(false, false), expected);
        assert_eq!(Backend::Glow.fallback(true, false), None);
        assert_eq!(Backend::Glow.fallback(false, true), None);
        assert_eq!(Backend::Dx12.fallback(false, false), None);
    }

    #[test]
    #[cfg(windows)]
    fn dx12_fallback_is_compiled_in() {
        assert!(eframe::wgpu::Instance::enabled_backend_features().contains(eframe::wgpu::Backends::DX12));
    }
}
