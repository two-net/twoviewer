//! The files made from the logo, `brand/logo.svg` (UI-01). `brand/README.md` says where each one
//! is used.

use std::{fs, path::PathBuf, process::ExitCode};

use anyhow::{Context, Result, bail, ensure};
use resvg::{
    tiny_skia::{Color, Pixmap, PixmapPaint, Transform},
    usvg,
};

const LOGO: &str = "brand/logo.svg";

/// The files made from the logo, relative to the repository root.
const ICONS: &[(&str, Icon)] = &[
    ("web/public/logo.svg", Icon::Copy),
    ("web/public/favicon.ico", Icon::Ico(&[16, 32, 48])),
    // iOS shows transparent pixels as black, and rounds the corners itself.
    ("web/public/apple-touch-icon.png", Icon::OpaquePng(180)),
    // Shown at 48 px, so sharp on screens with twice the pixel density. The corners stay
    // transparent, so the tile is rounded on light and dark backgrounds alike.
    ("server/templates/email/logo.png", Icon::Png(96)),
];

/// How much a color channel of a committed icon may differ from a fresh rendering, out of 255.
/// Rendering can differ slightly between processors (CI runs on x86, development often on ARM);
/// a changed logo differs by far more.
const TOLERANCE: u8 = 4;

enum Icon {
    /// The SVG itself.
    Copy,
    /// An ICO file with a PNG image for each size, in pixels.
    Ico(&'static [u32]),
    /// A PNG image of this size, in pixels.
    Png(u32),
    /// A PNG image of this size, in pixels, with transparent pixels in the tile's color.
    OpaquePng(u32),
}

/// `cargo xtask icons`: writes every file made from the logo.
pub fn write() -> Result<ExitCode> {
    let logo = Logo::load()?;
    for (path, icon) in ICONS {
        let bytes = match icon {
            Icon::Copy => logo.svg.clone(),
            _ => encode(icon, &logo.render(icon)?)?,
        };
        fs::write(root().join(path), bytes).with_context(|| format!("write {path}"))?;
        println!("Wrote {path}");
    }
    Ok(ExitCode::SUCCESS)
}

/// `cargo xtask check-icons`: fails if a file made from the logo is missing or out of date.
pub fn check() -> Result<ExitCode> {
    let logo = Logo::load()?;
    let mut stale = Vec::new();
    for (path, icon) in ICONS {
        let matches = match fs::read(root().join(path)) {
            Err(_) => false,
            Ok(bytes) => match icon {
                Icon::Copy => bytes == logo.svg,
                _ => decode(icon, &bytes).is_ok_and(|images| {
                    logo.render(icon)
                        .is_ok_and(|expected| same_images(&expected, &images))
                }),
            },
        };
        if !matches {
            stale.push(*path);
        }
    }
    if stale.is_empty() {
        println!("The icons match {LOGO}.");
        Ok(ExitCode::SUCCESS)
    } else {
        eprintln!("These files don't match {LOGO}; run `cargo xtask icons`:");
        for path in stale {
            eprintln!("  {path}");
        }
        Ok(ExitCode::FAILURE)
    }
}

struct Logo {
    svg: Vec<u8>,
    tree: usvg::Tree,
}

impl Logo {
    fn load() -> Result<Self> {
        let svg = fs::read(root().join(LOGO)).with_context(|| format!("read {LOGO}"))?;
        let tree = usvg::Tree::from_data(&svg, &usvg::Options::default())
            .with_context(|| format!("parse {LOGO}"))?;
        let size = tree.size();
        ensure!(size.width() == size.height(), "{LOGO} must be square");
        Ok(Self { svg, tree })
    }

    /// The images of `icon`, in the order they are stored.
    fn render(&self, icon: &Icon) -> Result<Vec<Pixmap>> {
        match icon {
            Icon::Copy => bail!("an SVG copy is not rendered"),
            Icon::Ico(sizes) => sizes.iter().map(|&size| self.render_at(size)).collect(),
            Icon::Png(size) => Ok(vec![self.render_at(*size)?]),
            Icon::OpaquePng(size) => Ok(vec![on_tile_color(&self.render_at(*size)?)?]),
        }
    }

    fn render_at(&self, size: u32) -> Result<Pixmap> {
        let mut image = Pixmap::new(size, size).context("image size")?;
        let scale = size as f32 / self.tree.size().width();
        resvg::render(
            &self.tree,
            Transform::from_scale(scale, scale),
            &mut image.as_mut(),
        );
        Ok(image)
    }
}

/// `image` on a background of the tile's color, so that its corners aren't transparent. The color
/// is taken from the image, so that it is written down only in the logo.
fn on_tile_color(image: &Pixmap) -> Result<Pixmap> {
    // The middle of the top edge is inside the tile, away from its rounded corners.
    let tile = image
        .pixel(image.width() / 2, 0)
        .context("empty image")?
        .demultiply();
    ensure!(
        tile.is_opaque(),
        "the tile of {LOGO} must reach the top edge"
    );
    let mut opaque = Pixmap::new(image.width(), image.height()).context("image size")?;
    opaque.fill(Color::from_rgba8(
        tile.red(),
        tile.green(),
        tile.blue(),
        255,
    ));
    opaque.draw_pixmap(
        0,
        0,
        image.as_ref(),
        &PixmapPaint::default(),
        Transform::identity(),
        None,
    );
    Ok(opaque)
}

/// The file holding `images`.
fn encode(icon: &Icon, images: &[Pixmap]) -> Result<Vec<u8>> {
    let pngs = images
        .iter()
        .map(Pixmap::encode_png)
        .collect::<Result<Vec<_>, _>>()?;
    match icon {
        Icon::Copy => bail!("an SVG copy is not encoded"),
        Icon::Png(_) | Icon::OpaquePng(_) => Ok(pngs.into_iter().next().context("no image")?),
        Icon::Ico(_) => {
            // ICONDIR, an ICONDIRENTRY per image, then the images.
            let mut ico = Vec::new();
            ico.extend(0u16.to_le_bytes()); // Reserved
            ico.extend(1u16.to_le_bytes()); // Type: icon
            ico.extend(u16::try_from(pngs.len())?.to_le_bytes());
            let mut offset = 6 + 16 * pngs.len();
            for (image, png) in images.iter().zip(&pngs) {
                // 0 means 256 pixels.
                ico.push(u8::try_from(image.width()).unwrap_or(0));
                ico.push(u8::try_from(image.height()).unwrap_or(0));
                ico.push(0); // Colors in the palette: none
                ico.push(0); // Reserved
                ico.extend(1u16.to_le_bytes()); // Color planes
                ico.extend(32u16.to_le_bytes()); // Bits per pixel
                ico.extend(u32::try_from(png.len())?.to_le_bytes());
                ico.extend(u32::try_from(offset)?.to_le_bytes());
                offset += png.len();
            }
            for png in pngs {
                ico.extend(png);
            }
            Ok(ico)
        }
    }
}

/// The images in a file written by [`encode`].
fn decode(icon: &Icon, bytes: &[u8]) -> Result<Vec<Pixmap>> {
    match icon {
        Icon::Copy => bail!("an SVG copy is not decoded"),
        Icon::Png(_) | Icon::OpaquePng(_) => Ok(vec![Pixmap::decode_png(bytes)?]),
        Icon::Ico(_) => {
            let u16_at = |at: usize| -> Result<usize> {
                let field = bytes.get(at..at + 2).context("truncated ICO file")?;
                Ok(u16::from_le_bytes(field.try_into()?).into())
            };
            let u32_at = |at: usize| -> Result<usize> {
                let field = bytes.get(at..at + 4).context("truncated ICO file")?;
                Ok(u32::from_le_bytes(field.try_into()?).try_into()?)
            };
            (0..u16_at(4)?)
                .map(|index| {
                    let entry = 6 + 16 * index;
                    let (len, offset) = (u32_at(entry + 8)?, u32_at(entry + 12)?);
                    let png = bytes
                        .get(offset..offset + len)
                        .context("truncated ICO file")?;
                    Ok(Pixmap::decode_png(png)?)
                })
                .collect()
        }
    }
}

fn same_images(expected: &[Pixmap], actual: &[Pixmap]) -> bool {
    expected.len() == actual.len()
        && expected.iter().zip(actual).all(|(expected, actual)| {
            expected.width() == actual.width()
                && expected.height() == actual.height()
                && expected
                    .data()
                    .iter()
                    .zip(actual.data())
                    .all(|(e, a)| e.abs_diff(*a) <= TOLERANCE)
        })
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_owned()
}
