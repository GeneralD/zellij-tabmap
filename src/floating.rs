//! Dependency-free floating-pane layer: config mode, hidden-float chips, and
//! (later) the visible-float overlay mapping. No zellij types, so the whole
//! module is unit-tested off-wasm (rule #8), exactly like `minimap`/`scroll`.

/// How the bar depicts a tab's floating-pane layer (config key `floating`, #110).
///
/// `Hybrid` is the B/A behaviour from the design: a tab whose floating layer is
/// visible overlays each float graphically on its tiled minimap, and a tab whose
/// layer is hidden shows one corner chip per float. `Off` restores the pre-#110
/// look — floating panes are invisible on the bar, exactly as before.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FloatingMode {
    /// Overlay visible floats; chip hidden floats — the default (#110).
    #[default]
    Hybrid,
    /// Draw no floating panes at all — the pre-#110 bar.
    Off,
}

impl std::str::FromStr for FloatingMode {
    type Err = ();

    /// `"hybrid"` / `"off"` (exact match); any other value errors so the config
    /// parser falls back to the documented default rather than panicking.
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "hybrid" => Ok(Self::Hybrid),
            "off" => Ok(Self::Off),
            _ => Err(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_floating_modes() {
        assert_eq!("hybrid".parse(), Ok(FloatingMode::Hybrid));
        assert_eq!("off".parse(), Ok(FloatingMode::Off));
    }

    #[test]
    fn malformed_floating_mode_errors() {
        // Case-sensitive, exact-match only — the config parser turns the error
        // into the documented default.
        assert_eq!("Hybrid".parse::<FloatingMode>(), Err(()));
        assert_eq!("chips".parse::<FloatingMode>(), Err(()));
        assert_eq!("".parse::<FloatingMode>(), Err(()));
    }

    #[test]
    fn default_is_hybrid() {
        assert_eq!(FloatingMode::default(), FloatingMode::Hybrid);
    }
}
