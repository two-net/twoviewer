//! The QR code that authenticator apps scan to set themselves up (AUTH-06).
//!
//! The web client draws it as an inline SVG path, since its Content-Security-Policy allows no
//! `data:` images, and so that no QR library runs in the browser. The server only encodes: the
//! path covers the dark modules, on a square of [`QrCode::size`] units that includes the quiet
//! zone that scanners need around the code.

use qrcode::{Color, EcLevel};
use serde::Serialize;
use zeroize::Zeroizing;

/// The light modules around the code that scanners need (ISO/IEC 18004).
const QUIET_ZONE: usize = 4;

/// A QR code, as an SVG path.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QrCode {
    /// The width and height of the code, quiet zone included, in modules: the SVG's `viewBox`
    /// is `0 0 size size`.
    pub size: usize,
    /// The dark modules, as SVG path data. It encodes the secret: never log it.
    #[serde(serialize_with = "serialize_path")]
    pub path: Zeroizing<String>,
}

impl std::fmt::Debug for QrCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QrCode")
            .field("size", &self.size)
            .finish_non_exhaustive()
    }
}

fn serialize_path<S: serde::Serializer>(
    path: &Zeroizing<String>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(path)
}

impl QrCode {
    /// The QR code of `text`, with medium error correction, which survives a scuffed screen and
    /// keeps the code small. `None` if `text` is too long for a QR code.
    pub fn encode(text: &str) -> Option<Self> {
        let code = qrcode::QrCode::with_error_correction_level(text, EcLevel::M).ok()?;
        let width = code.width();
        let colors = code.into_colors();
        let mut path = Zeroizing::new(String::new());
        for (y, row) in colors.chunks(width).enumerate() {
            let mut x = 0;
            while x < width {
                if row[x] == Color::Light {
                    x += 1;
                    continue;
                }
                // One rectangle per run of dark modules.
                let start = x;
                while x < width && row[x] == Color::Dark {
                    x += 1;
                }
                let (left, top, run) = (start + QUIET_ZONE, y + QUIET_ZONE, x - start);
                path.push_str(&format!("M{left} {top}h{run}v1h-{run}z"));
            }
        }
        Some(Self {
            size: width + 2 * QUIET_ZONE,
            path,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The dark modules that `path` draws, by parsing it back.
    fn dark_modules(code: &QrCode) -> Vec<Vec<bool>> {
        let mut modules = vec![vec![false; code.size]; code.size];
        for rectangle in code.path.split('M').filter(|r| !r.is_empty()) {
            let (left, rest) = rectangle.split_once(' ').unwrap();
            let (top, rest) = rest.split_once('h').unwrap();
            let (run, _) = rest.split_once('v').unwrap();
            let (left, top, run): (usize, usize, usize) = (
                left.parse().unwrap(),
                top.parse().unwrap(),
                run.parse().unwrap(),
            );
            for module in &mut modules[top][left..left + run] {
                assert!(!*module, "modules are drawn once");
                *module = true;
            }
        }
        modules
    }

    #[test]
    fn the_path_draws_the_dark_modules_inside_a_quiet_zone() {
        let text = "otpauth://totp/TwoViewer:ann%40example.com?secret=GEZDGNBVGY3TQOJQ";
        let reference = qrcode::QrCode::with_error_correction_level(text, EcLevel::M).unwrap();
        let width = reference.width();

        let code = QrCode::encode(text).unwrap();
        let modules = dark_modules(&code);

        assert_eq!(code.size, width + 8);
        for (y, row) in modules.iter().enumerate() {
            for (x, &dark) in row.iter().enumerate() {
                let inside = (4..4 + width).contains(&x) && (4..4 + width).contains(&y);
                let expected = inside && reference[(x - 4, y - 4)] == Color::Dark;
                assert_eq!(dark, expected, "({x}, {y})");
            }
        }
    }

    #[test]
    fn the_path_is_not_formatted() {
        let code = QrCode::encode("secret").unwrap();

        assert!(!format!("{code:?}").contains(&*code.path));
    }
}
