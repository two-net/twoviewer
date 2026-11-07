//! The colors of emails: the web client's (`web/src/styles.scss`), in light and dark mode
//! (THM-11). Every pair of colors used together meets WCAG 2.1 AA contrast in both, as in the web
//! client (THM-07); the tests below check it.
//!
//! `layout.html` sets each color inline in light mode, on an element with the class from [`ROLES`],
//! which switches it to dark mode in the clients that support that.

/// A color in light mode and in dark mode, as `#rrggbb`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Color {
    pub light: &'static str,
    pub dark: &'static str,
}

/// The page around the content.
pub const BG: Color = Color {
    light: "#ffffff",
    dark: "#121417",
};
/// The box that holds the content.
pub const SURFACE: Color = Color {
    light: "#f4f5f7",
    dark: "#1c1f24",
};
pub const TEXT: Color = Color {
    light: "#1b1d21",
    dark: "#e8eaed",
};
/// Secondary text, such as the footer.
pub const TEXT_MUTED: Color = Color {
    light: "#575c66",
    dark: "#a3a9b3",
};
/// The outline of the box, which needs no contrast.
pub const BORDER: Color = Color {
    light: "#d5d8de",
    dark: "#3a3f47",
};

/// The class that switches each color to dark mode, and the property it sets.
pub const ROLES: &[(&str, &str, Color)] = &[
    ("tv-bg", "background-color", BG),
    ("tv-surface", "background-color", SURFACE),
    ("tv-text", "color", TEXT),
    ("tv-text-muted", "color", TEXT_MUTED),
    ("tv-border", "border-color", BORDER),
];

#[cfg(test)]
mod tests {
    use super::*;

    const WEB_STYLES: &str = include_str!("../../../web/src/styles.scss");
    const LOGO: &str = include_str!("../../../brand/logo.svg");

    /// Text (WCAG 1.4.3).
    const TEXT_CONTRAST: f64 = 4.5;
    /// Graphics such as the logo (WCAG 1.4.11).
    const GRAPHIC_CONTRAST: f64 = 3.0;

    /// The relative luminance of a `#rrggbb` color (WCAG 2.1).
    fn luminance(color: &str) -> f64 {
        let channel = |start: usize| {
            let value =
                f64::from(u8::from_str_radix(&color[start..start + 2], 16).unwrap()) / 255.0;
            if value <= 0.04045 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        };
        assert!(color.len() == 7 && color.starts_with('#'), "{color}");
        0.2126 * channel(1) + 0.7152 * channel(3) + 0.0722 * channel(5)
    }

    /// The contrast ratio of two colors (WCAG 2.1), not rounded.
    fn contrast(a: &str, b: &str) -> f64 {
        let (a, b) = (luminance(a), luminance(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    #[test]
    fn the_colors_are_the_web_clients() {
        for (token, color) in [
            ("bg", BG),
            ("surface", SURFACE),
            ("text", TEXT),
            ("text-muted", TEXT_MUTED),
            ("border", BORDER),
        ] {
            let declaration = format!(
                "--color-{token}: light-dark({}, {});",
                color.light, color.dark
            );
            assert!(WEB_STYLES.contains(&declaration), "{declaration}");
        }
    }

    #[test]
    fn pairs_meet_wcag_aa_in_light_and_dark_mode() {
        // The logo's tile, the same in both modes.
        let tile = LOGO
            .split_once(r#"color=""#)
            .and_then(|(_, rest)| rest.split_once('"'))
            .map(|(color, _)| color)
            .unwrap();
        let tile = Color {
            light: tile,
            dark: tile,
        };
        for (foreground, background, minimum, used_for) in [
            (TEXT, SURFACE, TEXT_CONTRAST, "the content"),
            (TEXT, BG, TEXT_CONTRAST, "the name next to the logo"),
            (TEXT_MUTED, BG, TEXT_CONTRAST, "the footer"),
            (tile, BG, GRAPHIC_CONTRAST, "the logo"),
        ] {
            for (mode, foreground, background) in [
                ("light", foreground.light, background.light),
                ("dark", foreground.dark, background.dark),
            ] {
                let ratio = contrast(foreground, background);
                assert!(
                    ratio >= minimum,
                    "{used_for}, {mode}: {foreground} on {background} is {ratio:.2}:1"
                );
            }
        }
    }

    #[test]
    fn contrast_follows_wcag() {
        assert!((contrast("#000000", "#ffffff") - 21.0).abs() < 1e-9);
        assert!((contrast("#777777", "#777777") - 1.0).abs() < 1e-9);
    }
}
