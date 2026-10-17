//! The app's colours (U1), each with a value for the light theme and one
//! for the dark, chosen from the visuals in use. Every one is at least
//! 4.5:1 against its theme's panel (WCAG AA), as a test holds: the single
//! colours before were as low as 2.8:1 on the light panel.

use eframe::egui::{self, Color32};

/// A colour for each theme.
#[derive(Debug, Clone, Copy)]
pub struct Token {
    pub light: Color32,
    pub dark: Color32,
}

/// Something worked, or is on.
pub const GOOD: Token = Token {
    light: Color32::from_rgb(0x1e, 0x7a, 0x4f),
    dark: Color32::from_rgb(0x2e, 0x9e, 0x6a),
};

/// Something failed.
pub const BAD: Token = Token {
    light: Color32::from_rgb(0xb3, 0x3a, 0x2a),
    dark: Color32::from_rgb(0xd9, 0x5c, 0x4a),
};

/// Worth a look.
pub const WARN: Token = Token {
    light: Color32::from_rgb(0x8a, 0x5a, 0x00),
    dark: Color32::from_rgb(0xd9, 0x9a, 0x2b),
};

/// Hints and lesser text.
pub const MUTED: Token = Token {
    light: Color32::from_rgb(0x5f, 0x68, 0x66),
    dark: Color32::from_rgb(0x8a, 0x93, 0x91),
};

/// egui's own lesser text, the muted token in each theme (U1 review: egui's
/// "weak" text, the text colour at 0.6 alpha, is under 3:1 on the light
/// panel, and some 20 hints use it).
pub fn install(ctx: &egui::Context) {
    ctx.style_mut_of(egui::Theme::Light, |style| {
        style.visuals.weak_text_color = Some(MUTED.light);
    });
    ctx.style_mut_of(egui::Theme::Dark, |style| {
        style.visuals.weak_text_color = Some(MUTED.dark);
    });
}

/// `token` in the theme `ui` shows.
pub fn pick(ui: &egui::Ui, token: Token) -> Color32 {
    if ui.visuals().dark_mode {
        token.dark
    } else {
        token.light
    }
}

pub fn good(ui: &egui::Ui) -> Color32 {
    pick(ui, GOOD)
}

pub fn bad(ui: &egui::Ui) -> Color32 {
    pick(ui, BAD)
}

pub fn warn(ui: &egui::Ui) -> Color32 {
    pick(ui, WARN)
}

pub fn muted(ui: &egui::Ui) -> Color32 {
    pick(ui, MUTED)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// egui's panel fills, which the tokens are read against.
    const LIGHT_PANEL: Color32 = Color32::from_gray(248);
    const DARK_PANEL: Color32 = Color32::from_gray(27);

    /// WCAG's relative luminance of `c`.
    fn luminance(c: Color32) -> f64 {
        let channel = |v: u8| {
            let v = f64::from(v) / 255.0;
            if v <= 0.039_28 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(c.r()) + 0.7152 * channel(c.g()) + 0.0722 * channel(c.b())
    }

    /// WCAG's contrast ratio between two colours, 1 to 21.
    fn contrast(a: Color32, b: Color32) -> f64 {
        let (la, lb) = (luminance(a), luminance(b));
        (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
    }

    #[test]
    fn every_colour_reads_on_its_panel() {
        for (name, token) in [
            ("good", GOOD),
            ("bad", BAD),
            ("warn", WARN),
            ("muted", MUTED),
        ] {
            let light = contrast(token.light, LIGHT_PANEL);
            let dark = contrast(token.dark, DARK_PANEL);
            assert!(light >= 4.5, "{name} on the light panel: {light:.2}");
            assert!(dark >= 4.5, "{name} on the dark panel: {dark:.2}");
        }
    }

    #[test]
    fn the_contrast_is_wcags() {
        assert!((contrast(Color32::BLACK, Color32::WHITE) - 21.0).abs() < 0.01);
        assert!((contrast(Color32::WHITE, Color32::WHITE) - 1.0).abs() < 0.01);
    }
}
