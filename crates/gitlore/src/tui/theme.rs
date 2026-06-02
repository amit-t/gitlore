//! Centralised colour and style palette for the TUI (ADR-014).
//!
//! # Design
//!
//! [`Palette`] is the single source of truth for every colour used in the TUI.
//! Callers obtain a palette via [`resolve`], which:
//!
//! 1. Honours the user's `[tui] theme` config key (`dark`, `light`, `auto`).
//! 2. For `auto`, delegates to the injected [`TerminalProbe`] to detect the
//!    terminal background.  Falls back to `dark` if detection is unavailable.
//! 3. Swaps the risk-severity colours for the Wong colour-blind-safe set when
//!    `[tui] color_blind_safe = true`.
//!
//! # Colour choices (OQ-T-2, provisional)
//!
//! Dark variant uses Tailwind Slate tones; light variant inverts them.
//! Risk palette uses the Wong (2011) 8-colour set, which is safe for the most
//! common forms of colour-vision deficiency.
//!
//! NOTE: exact hex codes are provisional pending ADR-014 revision.

use gitlore_core::config::Theme;
use ratatui::style::Color;

// ---------------------------------------------------------------------------
// Palette
// ---------------------------------------------------------------------------

/// All colours used by the TUI, resolved once at startup.
#[derive(Debug, Clone, PartialEq)]
pub struct Palette {
    // ---- chrome ----
    /// Default background.
    pub bg: Color,
    /// Default foreground.
    pub fg: Color,
    /// Dimmed foreground for hints, footers, labels.
    pub fg_dim: Color,
    /// Accent / highlight (active tab, selected item).
    pub accent: Color,
    /// Border colour.
    pub border: Color,

    // ---- diff ----
    /// Added line background / foreground.
    pub diff_add_bg: Color,
    pub diff_add_fg: Color,
    /// Removed line background / foreground.
    pub diff_remove_bg: Color,
    pub diff_remove_fg: Color,
    /// Hunk header.
    pub diff_hunk_fg: Color,

    // ---- risk (Wong 2011 palette, colour-blind-safe) ----
    /// Low-risk label colour.
    pub risk_low: Color,
    /// Medium-risk label colour.
    pub risk_medium: Color,
    /// High-risk label colour.
    pub risk_high: Color,
}

impl Palette {
    /// Dark-mode palette (Tailwind Slate tones, provisional per OQ-T-2).
    fn dark(color_blind_safe: bool) -> Self {
        Self {
            bg: Color::Rgb(15, 23, 42),                // slate-900
            fg: Color::Rgb(226, 232, 240),             // slate-200
            fg_dim: Color::Rgb(100, 116, 139),         // slate-500
            accent: Color::Rgb(96, 165, 250),          // blue-400
            border: Color::Rgb(51, 65, 85),            // slate-700
            diff_add_bg: Color::Rgb(20, 83, 45),       // green-900
            diff_add_fg: Color::Rgb(134, 239, 172),    // green-300
            diff_remove_bg: Color::Rgb(127, 29, 29),   // red-900
            diff_remove_fg: Color::Rgb(252, 165, 165), // red-300
            diff_hunk_fg: Color::Rgb(147, 197, 253),   // blue-300
            ..risk_colours(color_blind_safe)
        }
    }

    /// Light-mode palette (Tailwind Slate tones inverted, provisional).
    fn light(color_blind_safe: bool) -> Self {
        Self {
            bg: Color::Rgb(248, 250, 252),             // slate-50
            fg: Color::Rgb(15, 23, 42),                // slate-900
            fg_dim: Color::Rgb(148, 163, 184),         // slate-400
            accent: Color::Rgb(37, 99, 235),           // blue-600
            border: Color::Rgb(203, 213, 225),         // slate-300
            diff_add_bg: Color::Rgb(220, 252, 231),    // green-100
            diff_add_fg: Color::Rgb(22, 101, 52),      // green-800
            diff_remove_bg: Color::Rgb(254, 226, 226), // red-100
            diff_remove_fg: Color::Rgb(153, 27, 27),   // red-800
            diff_hunk_fg: Color::Rgb(29, 78, 216),     // blue-700
            ..risk_colours(color_blind_safe)
        }
    }

    /// Monochromatic (no-color) palette: 16 ANSI tones only.
    pub fn mono() -> Self {
        Self {
            bg: Color::Reset,
            fg: Color::Reset,
            fg_dim: Color::DarkGray,
            accent: Color::White,
            border: Color::DarkGray,
            diff_add_bg: Color::Reset,
            diff_add_fg: Color::Green,
            diff_remove_bg: Color::Reset,
            diff_remove_fg: Color::Red,
            diff_hunk_fg: Color::Cyan,
            risk_low: Color::Green,
            risk_medium: Color::Yellow,
            risk_high: Color::Red,
        }
    }
}

/// Shared risk colours (independent of dark/light chrome).
///
/// When `color_blind_safe` is true, uses the Wong (2011) 8-colour set.
/// Otherwise uses intuitive traffic-light colours.
fn risk_colours(color_blind_safe: bool) -> Palette {
    if color_blind_safe {
        // Wong 2011: green=#009E73, orange=#E69F00, vermillion=#D55E00
        Palette {
            risk_low: Color::Rgb(0, 158, 115),
            risk_medium: Color::Rgb(230, 159, 0),
            risk_high: Color::Rgb(213, 94, 0),
            // Remaining fields are filled in by caller via struct update
            // syntax — these are placeholders that will be overwritten.
            bg: Color::Reset,
            fg: Color::Reset,
            fg_dim: Color::Reset,
            accent: Color::Reset,
            border: Color::Reset,
            diff_add_bg: Color::Reset,
            diff_add_fg: Color::Reset,
            diff_remove_bg: Color::Reset,
            diff_remove_fg: Color::Reset,
            diff_hunk_fg: Color::Reset,
        }
    } else {
        Palette {
            risk_low: Color::Rgb(74, 222, 128),    // green-400
            risk_medium: Color::Rgb(251, 191, 36), // amber-400
            risk_high: Color::Rgb(248, 113, 113),  // red-400
            bg: Color::Reset,
            fg: Color::Reset,
            fg_dim: Color::Reset,
            accent: Color::Reset,
            border: Color::Reset,
            diff_add_bg: Color::Reset,
            diff_add_fg: Color::Reset,
            diff_remove_bg: Color::Reset,
            diff_remove_fg: Color::Reset,
            diff_hunk_fg: Color::Reset,
        }
    }
}

// ---------------------------------------------------------------------------
// TerminalProbe trait
// ---------------------------------------------------------------------------

/// Abstraction over terminal-background detection (ADR-014).
///
/// The real implementation shells out to `termbg`; tests inject a
/// [`MockProbe`] that returns a deterministic answer without any I/O.
pub trait TerminalProbe {
    /// Return `true` when the terminal background is detected as light.
    /// Returns `false` when detection fails or the background is dark.
    fn is_light_background(&self) -> bool;
}

// ---------------------------------------------------------------------------
// SystemProbe
// ---------------------------------------------------------------------------

/// Production probe that calls [`termbg`] with a 200 ms timeout.
pub struct SystemProbe;

impl TerminalProbe for SystemProbe {
    fn is_light_background(&self) -> bool {
        // termbg::theme() queries the terminal via OSC escape sequences.
        // It requires a real TTY; in CI / test environments it returns an
        // error, which we map to `false` (dark) as the safe default.
        matches!(
            termbg::theme(std::time::Duration::from_millis(200)),
            Ok(termbg::Theme::Light)
        )
    }
}

// ---------------------------------------------------------------------------
// MockProbe
// ---------------------------------------------------------------------------

/// Deterministic probe for unit tests.
pub struct MockProbe {
    pub light: bool,
}

impl TerminalProbe for MockProbe {
    fn is_light_background(&self) -> bool {
        self.light
    }
}

// ---------------------------------------------------------------------------
// resolve
// ---------------------------------------------------------------------------

/// Build a [`Palette`] from a theme preference and a probe.
///
/// Resolution:
/// - `Theme::Dark`  → dark palette.
/// - `Theme::Light` → light palette.
/// - `Theme::Auto`  → probe the terminal; dark on failure.
pub fn resolve(theme: Theme, color_blind_safe: bool, probe: &dyn TerminalProbe) -> Palette {
    let is_light = match theme {
        Theme::Dark => false,
        Theme::Light => true,
        Theme::Auto => probe.is_light_background(),
    };
    if is_light {
        Palette::light(color_blind_safe)
    } else {
        Palette::dark(color_blind_safe)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dark_palette_has_dark_background() {
        let p = resolve(Theme::Dark, false, &MockProbe { light: false });
        // bg must be darker than midpoint (R+G+B < 384)
        match p.bg {
            Color::Rgb(r, g, b) => {
                let luma = r as u32 + g as u32 + b as u32;
                assert!(luma < 384, "dark bg too bright: luma={luma}");
            }
            other => panic!("expected Rgb for dark bg, got {other:?}"),
        }
    }

    #[test]
    fn light_palette_has_light_background() {
        let p = resolve(Theme::Light, false, &MockProbe { light: true });
        match p.bg {
            Color::Rgb(r, g, b) => {
                let luma = r as u32 + g as u32 + b as u32;
                assert!(luma > 600, "light bg too dark: luma={luma}");
            }
            other => panic!("expected Rgb for light bg, got {other:?}"),
        }
    }

    #[test]
    fn auto_follows_probe() {
        let dark = resolve(Theme::Auto, false, &MockProbe { light: false });
        let light = resolve(Theme::Auto, false, &MockProbe { light: true });
        // They must be different palettes.
        assert_ne!(dark.bg, light.bg);
    }

    #[test]
    fn color_blind_safe_uses_wong_risk_green() {
        // Wong green = #009E73 = RGB(0, 158, 115)
        let p = resolve(Theme::Dark, true, &MockProbe { light: false });
        assert_eq!(p.risk_low, Color::Rgb(0, 158, 115));
    }

    #[test]
    fn standard_risk_palette_uses_traffic_light() {
        let p = resolve(Theme::Dark, false, &MockProbe { light: false });
        // Standard: green-400 for low risk
        assert_eq!(p.risk_low, Color::Rgb(74, 222, 128));
    }

    #[test]
    fn mono_palette_uses_ansi_reset() {
        let p = Palette::mono();
        assert_eq!(p.bg, Color::Reset);
        assert_eq!(p.fg, Color::Reset);
    }

    #[test]
    fn dark_and_light_have_correct_diff_colours() {
        let dark = resolve(Theme::Dark, false, &MockProbe { light: false });
        let light = resolve(Theme::Light, false, &MockProbe { light: true });
        // Both should have non-Reset diff colours.
        assert_ne!(dark.diff_add_fg, Color::Reset);
        assert_ne!(light.diff_add_fg, Color::Reset);
        // Dark and light diff colours differ.
        assert_ne!(dark.diff_add_fg, light.diff_add_fg);
    }
}
