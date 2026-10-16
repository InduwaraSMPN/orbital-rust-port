//! Theme tables port of `App.xaml` + `Themes/Dark.xaml` + `Themes/Light.xaml` — the
//! 29 design-system resource entries (the port plan's "22 keys" undercounts the
//! XAML, see `PORT_STATUS.md` 2026-10-12), radii (8 / pill 16 / tooltip 14),
//! focus-ring accent + blur, button state colors. DWM accent blend-to-white 0.35
//! mutation stays runtime (`accent_with_soft`) in `app.rs`/`ui`.

/// One theme's resource table. Families follow the WPF keys (Bg, Surface, Text, …);
/// see `App.xaml` for the source names and the plan §4 table for the target layout.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ThemeColors {
    /// 29 resource entries, in `Themes/Dark.xaml` / `Themes/Light.xaml` declaration order.
    pub entries: [ThemeEntry; 29],
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ThemeEntry {
    pub key: &'static str,
    /// 0xAARRGGBB, matching the XAML hex values.
    pub value: u32,
}

impl ThemeColors {
    /// Looks a resource up by its WPF `x:Key` (None when the theme has no such key).
    pub fn get(&self, key: &str) -> Option<u32> {
        self.entries
            .iter()
            .find(|entry| entry.key == key)
            .map(|entry| entry.value)
    }

    /// Iterates the entries in XAML declaration order.
    pub fn iter(&self) -> impl Iterator<Item = &ThemeEntry> {
        self.entries.iter()
    }
}

pub const THEME_DARK: ThemeColors = ThemeColors {
    entries: [
        // ── Core design tokens ──
        ThemeEntry {
            key: "BgDeep",
            value: 0xFF202020,
        },
        ThemeEntry {
            key: "BgPanel",
            value: 0xFF2A2A2A,
        },
        ThemeEntry {
            key: "BgElement",
            value: 0xFF333333,
        },
        ThemeEntry {
            key: "BgHover",
            value: 0xFF3A3A3A,
        },
        ThemeEntry {
            key: "Border",
            value: 0xFF4A4A4A,
        },
        ThemeEntry {
            key: "AccentVi",
            value: 0xFF60A5FA,
        },
        ThemeEntry {
            key: "AccentCy",
            value: 0xFF93C5FD,
        },
        ThemeEntry {
            key: "TextHigh",
            value: 0xFFF3F3F3,
        },
        ThemeEntry {
            key: "TextMid",
            value: 0xFFD0D0D0,
        },
        ThemeEntry {
            key: "TextLow",
            value: 0xFFA0A0A0,
        },
        ThemeEntry {
            key: "Danger",
            value: 0xFFF87171,
        },
        // ── Settings window ──
        ThemeEntry {
            key: "WindowBgBrush",
            value: 0xFF202020,
        },
        ThemeEntry {
            key: "PanelBgBrush",
            value: 0xFF2A2A2A,
        },
        ThemeEntry {
            key: "PanelBorderBrush",
            value: 0xFF4A4A4A,
        },
        ThemeEntry {
            key: "LabelFgBrush",
            value: 0xFFA0A0A0,
        },
        ThemeEntry {
            key: "TitleFgBrush",
            value: 0xFFF3F3F3,
        },
        ThemeEntry {
            key: "SubtitleFgBrush",
            value: 0xFFD0D0D0,
        },
        ThemeEntry {
            key: "AccentBrush",
            value: 0xFF60A5FA,
        },
        // ── Popup pill ──
        ThemeEntry {
            key: "PillBgBrush",
            value: 0xD9303030,
        },
        ThemeEntry {
            key: "PillBorderBrush",
            value: 0x7060A5FA,
        },
        ThemeEntry {
            key: "PillGlowColor",
            value: 0xFF60A5FA,
        },
        ThemeEntry {
            key: "AccentViColor",
            value: 0xFF60A5FA,
        },
        ThemeEntry {
            key: "AccentCyColor",
            value: 0xFF93C5FD,
        },
        ThemeEntry {
            key: "BgPanelColor",
            value: 0xFF2A2A2A,
        },
        // ── Bar buttons ──
        ThemeEntry {
            key: "BtnFgBrush",
            value: 0xFFEFEFEF,
        },
        ThemeEntry {
            key: "BtnHoverBgBrush",
            value: 0x3060A5FA,
        },
        ThemeEntry {
            key: "BtnPressedBgBrush",
            value: 0x4060A5FA,
        },
        ThemeEntry {
            key: "BtnHoverFgBrush",
            value: 0xFF93C5FD,
        },
        ThemeEntry {
            key: "BtnDisabledFgBrush",
            value: 0x708A8A8A,
        },
    ],
};

pub const THEME_LIGHT: ThemeColors = ThemeColors {
    entries: [
        // ── Core design tokens (Claude Code Light palette) ──
        ThemeEntry {
            key: "BgDeep",
            value: 0xFFF3F3F3,
        },
        ThemeEntry {
            key: "BgPanel",
            value: 0xFFFFFFFF,
        },
        ThemeEntry {
            key: "BgElement",
            value: 0xFFFAFAFA,
        },
        ThemeEntry {
            key: "BgHover",
            value: 0xFFF0F0F0,
        },
        ThemeEntry {
            key: "Border",
            value: 0xFFDADADA,
        },
        ThemeEntry {
            key: "AccentVi",
            value: 0xFF2563EB,
        },
        ThemeEntry {
            key: "AccentCy",
            value: 0xFF60A5FA,
        },
        ThemeEntry {
            key: "TextHigh",
            value: 0xFF1F1F1F,
        },
        ThemeEntry {
            key: "TextMid",
            value: 0xFF4A4A4A,
        },
        ThemeEntry {
            key: "TextLow",
            value: 0xFF6E6E6E,
        },
        ThemeEntry {
            key: "Danger",
            value: 0xFFDC2626,
        },
        // ── Settings window ──
        ThemeEntry {
            key: "WindowBgBrush",
            value: 0xFFF3F3F3,
        },
        ThemeEntry {
            key: "PanelBgBrush",
            value: 0xFFFFFFFF,
        },
        ThemeEntry {
            key: "PanelBorderBrush",
            value: 0xFFDADADA,
        },
        ThemeEntry {
            key: "LabelFgBrush",
            value: 0xFF6E6E6E,
        },
        ThemeEntry {
            key: "TitleFgBrush",
            value: 0xFF1F1F1F,
        },
        ThemeEntry {
            key: "SubtitleFgBrush",
            value: 0xFF4A4A4A,
        },
        ThemeEntry {
            key: "AccentBrush",
            value: 0xFF2563EB,
        },
        // ── Popup pill ──
        ThemeEntry {
            key: "PillBgBrush",
            value: 0xF5FFFFFF,
        },
        ThemeEntry {
            key: "PillBorderBrush",
            value: 0x902563EB,
        },
        ThemeEntry {
            key: "PillGlowColor",
            value: 0xFF60A5FA,
        },
        ThemeEntry {
            key: "AccentViColor",
            value: 0xFF2563EB,
        },
        ThemeEntry {
            key: "AccentCyColor",
            value: 0xFF60A5FA,
        },
        ThemeEntry {
            key: "BgPanelColor",
            value: 0xFFFFFFFF,
        },
        // ── Bar buttons ──
        ThemeEntry {
            key: "BtnFgBrush",
            value: 0xFF1F1F1F,
        },
        ThemeEntry {
            key: "BtnHoverBgBrush",
            value: 0x1F2563EB,
        },
        ThemeEntry {
            key: "BtnPressedBgBrush",
            value: 0x332563EB,
        },
        ThemeEntry {
            key: "BtnHoverFgBrush",
            value: 0xFF2563EB,
        },
        ThemeEntry {
            key: "BtnDisabledFgBrush",
            value: 0x807E7E7E,
        },
    ],
};

pub fn colors(name: &str) -> ThemeColors {
    if name == "Light" {
        THEME_LIGHT
    } else {
        THEME_DARK
    }
}

/// Control corner radius — `App.xaml` TextBox / Button / ComboBox / ListView (8).
pub const RADIUS_CONTROL: f32 = 8.0;
/// Radial-menu bar-button radius — `RadialMenuWindow.xaml` (16). The pill's own
/// outer radius is 22 and is set inline in `ui::radial`.
pub const RADIUS_BAR_BUTTON: f32 = 16.0;
/// Result tooltip radius — `ResultTooltipWindow.xaml` borders (14).
pub const RADIUS_TOOLTIP: f32 = 14.0;

/// C# `App.TryGetSystemAccentColor` + `BlendTowards(accent, White, 0.35)`
/// (DwmGetColorizationColor, alpha 0 → 0xFF, Math.Round channels).
pub fn accent_with_soft() -> Option<(u32, u32)> {
    let raw = dwm_colorization_color()?;
    Some(accent_from_colorization(raw))
}

/// C# `TryGetSystemAccentColor` — alpha 0 becomes 0xFF, RGB bits pass through.
fn accent_from_colorization(raw: u32) -> (u32, u32) {
    let alpha = if (raw >> 24) & 0xFF == 0 {
        0xFF
    } else {
        (raw >> 24) & 0xFF
    };
    let red = ((raw >> 16) & 0xFF) as f64;
    let green = ((raw >> 8) & 0xFF) as f64;
    let blue = (raw & 0xFF) as f64;

    let (soft_red, soft_green, soft_blue) = blend_towards_white(red, green, blue, 0.35);

    let accent = (alpha << 24)
        | (((red as u32) & 0xFF) << 16)
        | (((green as u32) & 0xFF) << 8)
        | ((blue as u32) & 0xFF);
    let soft =
        0xFF00_0000 | ((soft_red as u32) << 16) | ((soft_green as u32) << 8) | (soft_blue as u32);

    (accent, soft)
}

/// `DwmGetColorizationColor` — None when the call fails (DWM disabled / non-Windows).
#[cfg(target_os = "windows")]
fn dwm_colorization_color() -> Option<u32> {
    let mut colorization: u32 = 0;
    // `BOOL` lives in the `windows_core` crate (not a direct dependency here), so
    // its type comes from the `FALSE` constant. The value is ignored, like the
    // source's `out _`.
    let mut opaque_blend = windows::Win32::Foundation::FALSE;

    match unsafe {
        windows::Win32::Graphics::Dwm::DwmGetColorizationColor(&mut colorization, &mut opaque_blend)
    } {
        Ok(()) => Some(colorization),
        Err(err) => {
            tracing::debug!(%err, "DwmGetColorizationColor failed; skipping system accent");
            None
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn dwm_colorization_color() -> Option<u32> {
    None
}

/// C# `BlendTowards(baseColor, White, ratio)` — ratio clamped to 0..1, channels
/// rounded with .NET `Math.Round` (midpoint to even) into bytes.
fn blend_towards_white(red: f64, green: f64, blue: f64, ratio: f64) -> (u8, u8, u8) {
    let ratio = math_clamp(ratio, 0.0, 1.0);
    (
        channel(math_round(red + ((255.0 - red) * ratio))),
        channel(math_round(green + ((255.0 - green) * ratio))),
        channel(math_round(blue + ((255.0 - blue) * ratio))),
    )
}

/// C# `(byte)Math.Round(...)`.
fn channel(value: f64) -> u8 {
    value.clamp(0.0, 255.0) as u8
}

/// C# `Math.Clamp(double, double, double)` — NaN in, NaN out.
fn math_clamp(value: f64, min: f64, max: f64) -> f64 {
    if value.is_nan() {
        return f64::NAN;
    }
    math_max(math_min(value, max), min)
}

/// C# `Math.Min(double, double)` — NaN in, NaN out.
fn math_min(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        return f64::NAN;
    }
    if a < b { a } else { b }
}

/// C# `Math.Max(double, double)` — NaN in, NaN out.
fn math_max(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        return f64::NAN;
    }
    if a > b { a } else { b }
}

/// C# `Math.Round(double)` — midpoint rounding to even (banker's rounding),
/// which is what the WPF `(byte)Math.Round(...)` casts in `BlendTowards` rely on.
fn math_round(value: f64) -> f64 {
    if value.is_nan() || value.is_infinite() {
        return value;
    }

    let floor = value.floor();
    let fraction = value - floor;

    if fraction < 0.5 {
        floor
    } else if fraction > 0.5 {
        floor + 1.0
    } else if (floor / 2.0).floor() * 2.0 == floor {
        // floor is even → round down
        floor
    } else {
        floor + 1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_defaults_to_dark_for_anything_but_light() {
        assert_eq!(colors("Dark"), THEME_DARK);
        assert_eq!(colors("Light"), THEME_LIGHT);
        assert_eq!(colors("System"), THEME_DARK);
        assert_eq!(colors(""), THEME_DARK);
    }

    #[test]
    fn both_themes_carry_all_29_keys_in_xaml_order() {
        let dark_keys: Vec<&str> = THEME_DARK.iter().map(|e| e.key).collect();
        let light_keys: Vec<&str> = THEME_LIGHT.iter().map(|e| e.key).collect();

        assert_eq!(dark_keys.len(), 29);
        assert_eq!(light_keys.len(), 29);
        assert_eq!(dark_keys, light_keys);
        assert_eq!(
            dark_keys,
            vec![
                "BgDeep",
                "BgPanel",
                "BgElement",
                "BgHover",
                "Border",
                "AccentVi",
                "AccentCy",
                "TextHigh",
                "TextMid",
                "TextLow",
                "Danger",
                "WindowBgBrush",
                "PanelBgBrush",
                "PanelBorderBrush",
                "LabelFgBrush",
                "TitleFgBrush",
                "SubtitleFgBrush",
                "AccentBrush",
                "PillBgBrush",
                "PillBorderBrush",
                "PillGlowColor",
                "AccentViColor",
                "AccentCyColor",
                "BgPanelColor",
                "BtnFgBrush",
                "BtnHoverBgBrush",
                "BtnPressedBgBrush",
                "BtnHoverFgBrush",
                "BtnDisabledFgBrush",
            ]
        );
    }

    #[test]
    fn dark_theme_matches_the_xaml_hex_values() {
        let expected: &[(&str, u32)] = &[
            ("BgDeep", 0xFF202020),
            ("BgPanel", 0xFF2A2A2A),
            ("BgElement", 0xFF333333),
            ("BgHover", 0xFF3A3A3A),
            ("Border", 0xFF4A4A4A),
            ("AccentVi", 0xFF60A5FA),
            ("AccentCy", 0xFF93C5FD),
            ("TextHigh", 0xFFF3F3F3),
            ("TextMid", 0xFFD0D0D0),
            ("TextLow", 0xFFA0A0A0),
            ("Danger", 0xFFF87171),
            ("WindowBgBrush", 0xFF202020),
            ("PanelBgBrush", 0xFF2A2A2A),
            ("PanelBorderBrush", 0xFF4A4A4A),
            ("LabelFgBrush", 0xFFA0A0A0),
            ("TitleFgBrush", 0xFFF3F3F3),
            ("SubtitleFgBrush", 0xFFD0D0D0),
            ("AccentBrush", 0xFF60A5FA),
            ("PillBgBrush", 0xD9303030),
            ("PillBorderBrush", 0x7060A5FA),
            ("PillGlowColor", 0xFF60A5FA),
            ("AccentViColor", 0xFF60A5FA),
            ("AccentCyColor", 0xFF93C5FD),
            ("BgPanelColor", 0xFF2A2A2A),
            ("BtnFgBrush", 0xFFEFEFEF),
            ("BtnHoverBgBrush", 0x3060A5FA),
            ("BtnPressedBgBrush", 0x4060A5FA),
            ("BtnHoverFgBrush", 0xFF93C5FD),
            ("BtnDisabledFgBrush", 0x708A8A8A),
        ];

        for (key, value) in expected {
            assert_eq!(THEME_DARK.get(key), Some(*value), "{key} mismatch");
        }
    }

    #[test]
    fn light_theme_matches_the_xaml_hex_values() {
        let expected: &[(&str, u32)] = &[
            ("BgDeep", 0xFFF3F3F3),
            ("BgPanel", 0xFFFFFFFF),
            ("BgElement", 0xFFFAFAFA),
            ("BgHover", 0xFFF0F0F0),
            ("Border", 0xFFDADADA),
            ("AccentVi", 0xFF2563EB),
            ("AccentCy", 0xFF60A5FA),
            ("TextHigh", 0xFF1F1F1F),
            ("TextMid", 0xFF4A4A4A),
            ("TextLow", 0xFF6E6E6E),
            ("Danger", 0xFFDC2626),
            ("WindowBgBrush", 0xFFF3F3F3),
            ("PanelBgBrush", 0xFFFFFFFF),
            ("PanelBorderBrush", 0xFFDADADA),
            ("LabelFgBrush", 0xFF6E6E6E),
            ("TitleFgBrush", 0xFF1F1F1F),
            ("SubtitleFgBrush", 0xFF4A4A4A),
            ("AccentBrush", 0xFF2563EB),
            ("PillBgBrush", 0xF5FFFFFF),
            ("PillBorderBrush", 0x902563EB),
            ("PillGlowColor", 0xFF60A5FA),
            ("AccentViColor", 0xFF2563EB),
            ("AccentCyColor", 0xFF60A5FA),
            ("BgPanelColor", 0xFFFFFFFF),
            ("BtnFgBrush", 0xFF1F1F1F),
            ("BtnHoverBgBrush", 0x1F2563EB),
            ("BtnPressedBgBrush", 0x332563EB),
            ("BtnHoverFgBrush", 0xFF2563EB),
            ("BtnDisabledFgBrush", 0x807E7E7E),
        ];

        for (key, value) in expected {
            assert_eq!(THEME_LIGHT.get(key), Some(*value), "{key} mismatch");
        }
    }

    #[test]
    fn get_returns_none_for_unknown_keys() {
        assert_eq!(THEME_DARK.get("NoSuchBrush"), None);
    }

    #[test]
    fn math_round_uses_midpoint_to_even() {
        assert_eq!(math_round(0.5), 0.0);
        assert_eq!(math_round(1.5), 2.0);
        assert_eq!(math_round(2.5), 2.0);
        assert_eq!(math_round(3.5), 4.0);
        assert_eq!(math_round(-0.5), 0.0);
        assert_eq!(math_round(-1.5), -2.0);
        assert_eq!(math_round(1.4), 1.0);
        assert_eq!(math_round(1.6), 2.0);
        assert_eq!(math_round(255.0), 255.0);
    }

    #[test]
    fn blend_towards_white_matches_dotnet_math() {
        // #2563EB → (37, 99, 235): 37 + (255-37)*0.35 = 113.3 → 113;
        // 99 + (255-99)*0.35 = 153.6 → 154; 235 + (255-235)*0.35 = 242.
        assert_eq!(
            blend_towards_white(37.0, 99.0, 235.0, 0.35),
            (113, 154, 242)
        );

        assert_eq!(blend_towards_white(0.0, 0.0, 0.0, 0.35), (89, 89, 89));
        assert_eq!(
            blend_towards_white(255.0, 255.0, 255.0, 0.35),
            (255, 255, 255)
        );
        assert_eq!(
            blend_towards_white(128.0, 128.0, 128.0, 0.0),
            (128, 128, 128)
        );
        assert_eq!(
            blend_towards_white(128.0, 128.0, 128.0, 1.0),
            (255, 255, 255)
        );
    }

    #[test]
    fn accent_from_colorization_forces_alpha_and_blends() {
        // DWM packs ARGB; alpha 0 becomes 0xFF, RGB passes through untouched.
        let (accent, soft) = accent_from_colorization(0x002563EB);
        assert_eq!(accent, 0xFF2563EB);

        // blended: 37→113, 99→154, 235→242 (see blend test above)
        assert_eq!(soft, 0xFF719AF2);

        let (accent, soft) = accent_from_colorization(0xFF000000);
        assert_eq!(accent, 0xFF000000);
        assert_eq!(soft, 0xFF595959); // 0 + 255*0.35 = 89.25 → 89
    }

    #[test]
    fn accent_with_soft_either_fails_or_is_well_formed() {
        // Headless/service sessions have no DWM accent — None is the source's
        // behavior too (TryGetSystemAccentColor returns false).
        if let Some((accent, soft)) = accent_with_soft() {
            // The soft accent is always fully opaque; the accent keeps the DWM
            // coverage byte unless it was 0 (forced to 0xFF by the port).
            assert_eq!(soft >> 24, 0xFF);
            assert_ne!(accent >> 24, 0);

            let (r, g, b) = (
                ((soft >> 16) & 0xFF) as f64,
                ((soft >> 8) & 0xFF) as f64,
                (soft & 0xFF) as f64,
            );
            let expected = blend_towards_white(
                ((accent >> 16) & 0xFF) as f64,
                ((accent >> 8) & 0xFF) as f64,
                (accent & 0xFF) as f64,
                0.35,
            );
            assert_eq!(
                (r, g, b),
                (expected.0 as f64, expected.1 as f64, expected.2 as f64)
            );
        }
    }
}
