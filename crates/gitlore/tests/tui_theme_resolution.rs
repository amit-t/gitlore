//! Integration test: theme resolution end-to-end (AC-TUI-3 / ADR-014).
//!
//! Tests that the `[tui] theme` config key and `color_blind_safe` flag are
//! honoured by the resolved [`Palette`].  No real terminal is needed; we use
//! [`MockProbe`] directly.

use gitlore::tui::theme::{resolve, MockProbe, Palette};
use gitlore_core::config::Theme;
use ratatui::style::Color;

#[test]
fn dark_theme_explicit_returns_dark_palette() {
    let p = resolve(Theme::Dark, false, &MockProbe { light: false });
    match p.bg {
        Color::Rgb(r, g, b) => {
            let luma = r as u32 + g as u32 + b as u32;
            assert!(luma < 384, "dark bg luma too high: {luma}");
        }
        other => panic!("expected Rgb bg, got {other:?}"),
    }
}

#[test]
fn light_theme_explicit_returns_light_palette() {
    let p = resolve(Theme::Light, false, &MockProbe { light: false });
    match p.bg {
        Color::Rgb(r, g, b) => {
            let luma = r as u32 + g as u32 + b as u32;
            assert!(luma > 600, "light bg luma too low: {luma}");
        }
        other => panic!("expected Rgb bg, got {other:?}"),
    }
}

#[test]
fn auto_theme_uses_probe_dark() {
    let p = resolve(Theme::Auto, false, &MockProbe { light: false });
    // Same result as explicit Dark.
    let p_dark = resolve(Theme::Dark, false, &MockProbe { light: false });
    assert_eq!(p.bg, p_dark.bg);
}

#[test]
fn auto_theme_uses_probe_light() {
    let p = resolve(Theme::Auto, false, &MockProbe { light: true });
    let p_light = resolve(Theme::Light, false, &MockProbe { light: true });
    assert_eq!(p.bg, p_light.bg);
}

#[test]
fn color_blind_safe_flag_changes_risk_palette() {
    let normal = resolve(Theme::Dark, false, &MockProbe { light: false });
    let safe = resolve(Theme::Dark, true, &MockProbe { light: false });
    // Wong green vs traffic-light green must differ.
    assert_ne!(
        normal.risk_low, safe.risk_low,
        "color_blind_safe must change risk_low colour"
    );
    // Wong green = RGB(0, 158, 115).
    assert_eq!(safe.risk_low, Color::Rgb(0, 158, 115));
}

#[test]
fn mono_palette_is_independent_of_probe() {
    let p = Palette::mono();
    assert_eq!(p.bg, Color::Reset);
    assert_eq!(p.fg, Color::Reset);
}
