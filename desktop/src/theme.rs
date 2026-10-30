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
    dark: Color32::from_rgb(0xe2, 0x6a, 0x58),
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

/// The size of small text: the hints under settings, the sections' names
/// down the left, a text's status (AM4: egui's own 9 points read too small
/// in a screenshot of every page).
pub const SMALL_TEXT: f32 = 11.0;

/// A page's heading (UX 3): a step above egui's 18, so a page's name and
/// a section's differ at a glance.
pub const HEADING_TEXT: f32 = 20.0;

/// A section's heading inside a page (UX 3), drawn by [`section`].
pub const SECTION_TEXT: f32 = 15.0;

/// An empty page's card title (UX 4): a step above body text, as the
/// bundled fonts have no bold for `strong` to use.
pub const CARD_TITLE_TEXT: f32 = 14.0;

/// The one accent (UX 3): the keyboard focus ring, a focused text box's
/// frame and the selected text. Read at 4.5:1 on the panel and on
/// [`ACCENT_FILL`], as a test holds.
pub const ACCENT: Token = Token {
    light: Color32::from_rgb(0x1d, 0x5c, 0xc0),
    dark: Color32::from_rgb(0x7a, 0xb2, 0xff),
};

/// The accent as a fill: behind what is selected (the page down the left,
/// selected text), with [`ACCENT`] or the text colour on it.
pub const ACCENT_FILL: Token = Token {
    light: Color32::from_rgb(0xd6, 0xe4, 0xfb),
    dark: Color32::from_rgb(0x1c, 0x34, 0x5c),
};

/// A card's fill (UX 3), a step off the panel's.
pub const CARD: Token = Token {
    light: Color32::from_gray(255),
    dark: Color32::from_gray(34),
};

/// A card's edge.
pub const CARD_EDGE: Token = Token {
    light: Color32::from_gray(218),
    dark: Color32::from_gray(58),
};

/// The text of a label, a button and a checkbox, in each theme (UX 3):
/// darker in the light theme and lighter in the dark than egui's, so that
/// greyed out ([`DISABLED_ALPHA`]) they still read at 4.5:1.
const TEXT: Token = Token {
    light: Color32::from_gray(36),
    dark: Color32::from_gray(196),
};
const BUTTON_TEXT: Token = Token {
    light: Color32::from_gray(30),
    dark: Color32::from_gray(222),
};

/// The edge of a button or a checkbox's box at rest (UX 3): at 3:1 on
/// the panel and on a card, as a control's edge must be (WCAG 1.4.11);
/// egui's has none, and its light box was 1.1:1 on the panel.
const EDGE: Token = Token {
    light: Color32::from_gray(138),
    dark: Color32::from_gray(110),
};

/// How much a greyed-out control keeps (egui's own is 0.5, which left
/// disabled text at under 3:1 in both themes).
pub const DISABLED_ALPHA: f32 = 0.7;

/// The corner of a button, a box and a checkbox (UX 3).
pub const CORNER: u8 = 6;

/// The width of the focus ring (UX 3: egui's is 1 px, and faint).
pub const FOCUS_WIDTH: f32 = 2.0;

/// The app's look, set once for both themes:
/// - lesser text in the muted token (U1 review: egui's "weak" text, the text
///   colour at 0.6 alpha, is under 3:1 on the light panel), small text at
///   [`SMALL_TEXT`];
/// - UX 3's visual system: room between things (8×6), buttons 26 px high
///   with 10×4 padding, corners of 6, one accent for what is selected and
///   what has the keyboard's focus (a 2 px ring), greyed-out text at 4.5:1
///   or more, and a page heading at [`HEADING_TEXT`].
pub fn install(ctx: &egui::Context) {
    for theme in [egui::Theme::Light, egui::Theme::Dark] {
        let pick = |token: Token| match theme {
            egui::Theme::Light => token.light,
            egui::Theme::Dark => token.dark,
        };
        ctx.style_mut_of(theme, |style| {
            if let Some(small) = style.text_styles.get_mut(&egui::TextStyle::Small) {
                small.size = SMALL_TEXT;
            }
            if let Some(heading) = style.text_styles.get_mut(&egui::TextStyle::Heading) {
                heading.size = HEADING_TEXT;
            }
            let spacing = &mut style.spacing;
            spacing.item_spacing = egui::vec2(8.0, 6.0);
            spacing.button_padding = egui::vec2(10.0, 4.0);
            spacing.interact_size.y = 26.0;
            // A checkbox's box big enough that its corners leave it a
            // square, not a dot like a radio button's.
            spacing.icon_width = 18.0;
            spacing.icon_width_inner = 10.0;

            let visuals = &mut style.visuals;
            visuals.weak_text_color = Some(pick(MUTED));
            visuals.disabled_alpha = DISABLED_ALPHA;
            let accent = pick(ACCENT);
            visuals.selection.bg_fill = pick(ACCENT_FILL);
            visuals.selection.stroke = egui::Stroke::new(FOCUS_WIDTH, accent);
            visuals.hyperlink_color = accent;
            visuals.window_corner_radius = egui::CornerRadius::same(CORNER + 2);
            visuals.menu_corner_radius = egui::CornerRadius::same(CORNER + 2);
            visuals.widgets.noninteractive.fg_stroke.color = pick(TEXT);
            let widgets = &mut visuals.widgets;
            for state in [
                &mut widgets.noninteractive,
                &mut widgets.inactive,
                &mut widgets.hovered,
                &mut widgets.active,
                &mut widgets.open,
            ] {
                state.corner_radius = egui::CornerRadius::same(CORNER);
            }
            widgets.inactive.fg_stroke.color = pick(BUTTON_TEXT);
            widgets.inactive.bg_stroke = egui::Stroke::new(1.0, pick(EDGE));
            widgets.open.fg_stroke.color = pick(BUTTON_TEXT);
            // A control with the keyboard's focus (and one held down) is
            // drawn as "active": its ring is the accent's, 2 px.
            widgets.active.bg_stroke = egui::Stroke::new(FOCUS_WIDTH, accent);
        });
    }
}

/// A section's heading inside a page (UX 3), at [`SECTION_TEXT`].
pub fn section(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.label(egui::RichText::new(text).size(SECTION_TEXT).strong())
}

/// A card (UX 3): a device's or a computer's row, a page's status. The
/// card's fill and edge are a step off the panel's; it takes the width
/// it is given.
pub fn card<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::new()
        .fill(pick(ui, CARD))
        .stroke(egui::Stroke::new(1.0, pick(ui, CARD_EDGE)))
        .corner_radius(egui::CornerRadius::same(CORNER + 2))
        .inner_margin(egui::Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

/// A coloured dot (UX 3), as a device's card and the status card show
/// where it stands. Painted: the bundled fonts have no "●" glyph.
pub fn dot(ui: &mut egui::Ui, token: Token) {
    let color = pick(ui, token);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 5.0, color);
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

/// The text on a chip filled with `fill` (I3): black or white, whichever
/// reads better on it.
pub fn on_chip(_ui: &egui::Ui, fill: Color32) -> Color32 {
    chip_text(fill)
}

fn chip_text(fill: Color32) -> Color32 {
    if contrast(Color32::BLACK, fill) >= contrast(Color32::WHITE, fill) {
        Color32::BLACK
    } else {
        Color32::WHITE
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    /// AM4: a hint, a section's name down the left and a status under a
    /// text read at 11 points in both themes, not egui's 9.
    #[test]
    fn small_text_is_readable() {
        let ctx = egui::Context::default();
        install(&ctx);
        for theme in [egui::Theme::Light, egui::Theme::Dark] {
            let size = ctx.style_of(theme).text_styles[&egui::TextStyle::Small].size;
            assert!(size >= SMALL_TEXT, "{theme:?}: {size}");
        }
    }

    /// egui's panel fills, which the tokens are read against.
    const LIGHT_PANEL: Color32 = Color32::from_gray(248);
    const DARK_PANEL: Color32 = Color32::from_gray(27);

    /// I3: a status chip's word reads on its fill, in both themes.
    #[test]
    fn a_chips_text_reads_on_it() {
        for token in [GOOD, BAD, WARN, MUTED] {
            for fill in [token.light, token.dark] {
                let ratio = contrast(chip_text(fill), fill);
                assert!(ratio >= 4.5, "{fill:?}: {ratio:.2}");
            }
        }
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

    /// `fg` drawn at `alpha` over `under`, as a greyed-out control's
    /// painter blends it.
    fn faded(fg: Color32, under: Color32, alpha: f32) -> Color32 {
        let mix =
            |f: u8, u: u8| (f32::from(f) * alpha + f32::from(u) * (1.0 - alpha)).round() as u8;
        Color32::from_rgb(
            mix(fg.r(), under.r()),
            mix(fg.g(), under.g()),
            mix(fg.b(), under.b()),
        )
    }

    /// UX 3: a greyed-out label, and a greyed-out button's text on its
    /// own fill, read at 4.5:1 in both themes, on the panel and on a card.
    #[test]
    fn greyed_out_text_still_reads() {
        let ctx = egui::Context::default();
        install(&ctx);
        for (theme, panel, card) in [
            (egui::Theme::Light, LIGHT_PANEL, CARD.light),
            (egui::Theme::Dark, DARK_PANEL, CARD.dark),
        ] {
            let style = ctx.style_of(theme);
            let visuals = &style.visuals;
            let alpha = visuals.disabled_alpha;
            for under in [panel, card] {
                let label = faded(visuals.widgets.noninteractive.fg_stroke.color, under, alpha);
                let ratio = contrast(label, under);
                assert!(ratio >= 4.5, "{theme:?} label on {under:?}: {ratio:.2}");
                let fill = faded(visuals.widgets.inactive.weak_bg_fill, under, alpha);
                let text = faded(visuals.widgets.inactive.fg_stroke.color, under, alpha);
                let ratio = contrast(text, fill);
                assert!(ratio >= 4.5, "{theme:?} button on {under:?}: {ratio:.2}");
            }
        }
    }

    /// UX 3: the accent reads on the panel (a focus ring, a link) and on
    /// its own fill (the page selected down the left); text reads on that
    /// fill too (selected text).
    #[test]
    fn the_accent_reads() {
        let ctx = egui::Context::default();
        install(&ctx);
        for (theme, panel, accent, fill) in [
            (
                egui::Theme::Light,
                LIGHT_PANEL,
                ACCENT.light,
                ACCENT_FILL.light,
            ),
            (egui::Theme::Dark, DARK_PANEL, ACCENT.dark, ACCENT_FILL.dark),
        ] {
            let style = ctx.style_of(theme);
            assert_eq!(style.visuals.selection.bg_fill, fill);
            assert_eq!(style.visuals.selection.stroke.color, accent);
            assert_eq!(style.visuals.widgets.active.bg_stroke.color, accent);
            assert!(style.visuals.widgets.active.bg_stroke.width >= FOCUS_WIDTH);
            for (what, ratio) in [
                ("accent on the panel", contrast(accent, panel)),
                ("accent on its fill", contrast(accent, fill)),
                (
                    "text on the fill",
                    contrast(style.visuals.widgets.noninteractive.fg_stroke.color, fill),
                ),
            ] {
                assert!(ratio >= 4.5, "{theme:?} {what}: {ratio:.2}");
            }
        }
    }

    /// UX 3: a control's edge at rest shows at 3:1, on the panel and on
    /// a card, in both themes.
    #[test]
    fn a_controls_edge_shows() {
        let ctx = egui::Context::default();
        install(&ctx);
        for (theme, panel, card) in [
            (egui::Theme::Light, LIGHT_PANEL, CARD.light),
            (egui::Theme::Dark, DARK_PANEL, CARD.dark),
        ] {
            let edge = ctx.style_of(theme).visuals.widgets.inactive.bg_stroke;
            assert!(edge.width >= 1.0);
            for under in [panel, card] {
                let ratio = contrast(edge.color, under);
                assert!(ratio >= 3.0, "{theme:?} on {under:?}: {ratio:.2}");
            }
        }
    }

    /// UX 3: the status colours read on a card as they do on the panel.
    #[test]
    fn every_colour_reads_on_a_card() {
        for token in [GOOD, BAD, WARN, MUTED, ACCENT] {
            let light = contrast(token.light, CARD.light);
            let dark = contrast(token.dark, CARD.dark);
            assert!(light >= 4.5, "{token:?} on a light card: {light:.2}");
            assert!(dark >= 4.5, "{token:?} on a dark card: {dark:.2}");
        }
    }

    /// UX 3's sizes: buttons a finger's height, a page's heading above a
    /// section's.
    #[test]
    fn the_sizes_are_set() {
        let ctx = egui::Context::default();
        install(&ctx);
        for theme in [egui::Theme::Light, egui::Theme::Dark] {
            let style = ctx.style_of(theme);
            assert_eq!(style.spacing.interact_size.y, 26.0);
            assert_eq!(style.spacing.button_padding, egui::vec2(10.0, 4.0));
            assert_eq!(style.spacing.item_spacing, egui::vec2(8.0, 6.0));
            assert_eq!(
                style.visuals.widgets.inactive.corner_radius,
                egui::CornerRadius::same(CORNER)
            );
            let heading = style.text_styles[&egui::TextStyle::Heading].size;
            assert!(heading > SECTION_TEXT, "{heading}");
        }
    }

    #[test]
    fn the_contrast_is_wcags() {
        assert!((contrast(Color32::BLACK, Color32::WHITE) - 21.0).abs() < 0.01);
        assert!((contrast(Color32::WHITE, Color32::WHITE) - 1.0).abs() < 0.01);
    }
}
