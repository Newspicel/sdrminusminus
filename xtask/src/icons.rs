use std::path::Path;

use anyhow::{Context, Result};
use resvg::{tiny_skia, usvg};

const SOURCE: &str = "assets/icon.svg";

const ICO_SIZES: &[u32] = &[16, 24, 32, 48, 64, 128, 256];

const FAVICON_SIZES: &[u32] = &[16, 32, 48];

const ICNS_ENTRIES: &[(&[u8; 4], u32)] = &[
    (b"ic11", 32),
    (b"ic12", 64),
    (b"ic07", 128),
    (b"ic13", 256),
    (b"ic08", 256),
    (b"ic14", 512),
    (b"ic09", 512),
    (b"ic10", 1024),
];

const MACOS_SCALE: f32 = 824.0 / 1024.0;

pub fn icons(root: &Path) -> Result<()> {
    let source = root.join(SOURCE);
    let svg = std::fs::read(&source).with_context(|| format!("read {}", source.display()))?;
    let tree = usvg::Tree::from_data(&svg, &usvg::Options::default())
        .with_context(|| format!("parse {}", source.display()))?;

    let copies = [
        root.join("web/public/icon.svg"),
        root.join("docs/src/icon.svg"),
        root.join("docs/theme/favicon.svg"),
    ];
    for path in &copies {
        write(path, &svg)?;
    }

    let pngs = [
        ("apps/desktop/icons/32x32.png", 32),
        ("apps/desktop/icons/128x128.png", 128),
        ("apps/desktop/icons/128x128@2x.png", 256),
        ("apps/desktop/icons/icon.png", 512),
        ("docs/theme/favicon.png", 32),
    ];
    for (path, size) in pngs {
        write(&root.join(path), &render(&tree, size, 1.0)?)?;
    }

    write(
        &root.join("apps/desktop/icons/icon.ico"),
        &ico(&tree, ICO_SIZES)?,
    )?;
    write(
        &root.join("web/public/favicon.ico"),
        &ico(&tree, FAVICON_SIZES)?,
    )?;
    write(&root.join("apps/desktop/icons/icon.icns"), &icns(&tree)?)?;
    Ok(())
}

fn raster(tree: &usvg::Tree, size: u32, fill: f32) -> Result<tiny_skia::Pixmap> {
    let mut pixmap = tiny_skia::Pixmap::new(size, size).context("allocate pixmap")?;
    #[expect(
        clippy::cast_precision_loss,
        reason = "icon sizes are three-digit integers; exact in f32"
    )]
    let canvas = size as f32;
    let target = canvas * fill;
    let scale = target / tree.size().width();
    let offset = (canvas - target) / 2.0;
    resvg::render(
        tree,
        tiny_skia::Transform::from_translate(offset, offset).pre_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    Ok(pixmap)
}

fn render(tree: &usvg::Tree, size: u32, fill: f32) -> Result<Vec<u8>> {
    raster(tree, size, fill)?.encode_png().context("encode PNG")
}

fn ico(tree: &usvg::Tree, sizes: &[u32]) -> Result<Vec<u8>> {
    const PNG_FROM: u32 = 256;
    let images = sizes
        .iter()
        .map(|&size| {
            let pixmap = raster(tree, size, 1.0)?;
            if size >= PNG_FROM {
                return pixmap.encode_png().context("encode PNG");
            }
            Ok(dib(&pixmap))
        })
        .collect::<Result<Vec<_>>>()?;

    let mut out = Vec::new();
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&u16::try_from(images.len())?.to_le_bytes());

    const HEADER: usize = 6;
    const ENTRY: usize = 16;
    let mut offset = HEADER + ENTRY * images.len();
    for (&size, image) in sizes.iter().zip(&images) {
        let dim = u8::try_from(size % 256).context("ico entry larger than 256px")?;
        out.push(dim);
        out.push(dim);
        out.push(0);
        out.push(0);
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&32u16.to_le_bytes());
        out.extend_from_slice(&u32::try_from(image.len())?.to_le_bytes());
        out.extend_from_slice(&u32::try_from(offset)?.to_le_bytes());
        offset += image.len();
    }
    for image in &images {
        out.extend_from_slice(image);
    }
    Ok(out)
}

fn dib(pixmap: &tiny_skia::Pixmap) -> Vec<u8> {
    let (width, height) = (pixmap.width(), pixmap.height());
    let mut out = Vec::new();
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&width.to_le_bytes());
    out.extend_from_slice(&(height * 2).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&(width * height * 4).to_le_bytes());
    out.extend_from_slice(&[0; 16]);

    let (width, height) = (width as usize, height as usize);
    let pixels = pixmap.pixels();
    for row in (0..height).rev() {
        for column in 0..width {
            let pixel = pixels[row * width + column].demultiply();
            out.extend_from_slice(&[pixel.blue(), pixel.green(), pixel.red(), pixel.alpha()]);
        }
    }
    out.resize(
        out.len() + width.div_ceil(8).next_multiple_of(4) * height,
        0,
    );
    out
}

fn icns(tree: &usvg::Tree) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    for &(kind, size) in ICNS_ENTRIES {
        let png = render(tree, size, MACOS_SCALE)?;
        body.extend_from_slice(kind);
        body.extend_from_slice(&(u32::try_from(png.len())? + 8).to_be_bytes());
        body.extend_from_slice(&png);
    }

    let mut out = Vec::with_capacity(body.len() + 8);
    out.extend_from_slice(b"icns");
    out.extend_from_slice(&(u32::try_from(body.len())? + 8).to_be_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}

fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("icon path has no directory")?;
    std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    std::fs::write(path, bytes).with_context(|| format!("write {}", path.display()))?;
    println!("wrote {} ({} bytes)", path.display(), bytes.len());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> usvg::Tree {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("xtask has a parent")
            .join(SOURCE);
        let svg = std::fs::read(&root).expect("the committed mark");
        usvg::Tree::from_data(&svg, &usvg::Options::default()).expect("the mark parses")
    }

    fn be32(bytes: &[u8], at: usize) -> u32 {
        u32::from_be_bytes(bytes[at..at + 4].try_into().expect("four bytes"))
    }

    fn le32(bytes: &[u8], at: usize) -> u32 {
        u32::from_le_bytes(bytes[at..at + 4].try_into().expect("four bytes"))
    }

    #[test]
    fn renders_a_square_png_of_the_asked_size() {
        let png = render(&tree(), 32, 1.0).expect("render");
        assert_eq!(&png[1..4], b"PNG");
        assert_eq!(be32(&png, 16), 32);
        assert_eq!(be32(&png, 20), 32);
    }

    #[test]
    fn ico_entries_are_dibs_below_256_and_a_png_at_it() {
        let sizes = [16, 32, 256];
        let ico = ico(&tree(), &sizes).expect("ico");
        assert_eq!(u16::from_le_bytes([ico[4], ico[5]]), 3);

        for (i, &size) in sizes.iter().enumerate() {
            let entry = 6 + 16 * i;
            let expected = u8::try_from(size % 256).expect("dimension byte");
            assert_eq!(ico[entry], expected, "width of the {size}px entry");
            assert_eq!(ico[entry + 1], expected, "height of the {size}px entry");

            let len = le32(&ico, entry + 8) as usize;
            let at = le32(&ico, entry + 12) as usize;
            assert!(at + len <= ico.len(), "{size}px entry fits in the file");

            if size >= 256 {
                assert_eq!(&ico[at + 1..at + 4], b"PNG", "the 256px entry is a PNG");
                assert_eq!(be32(&ico, at + 16), size, "PNG entry is that size");
                continue;
            }
            assert_eq!(le32(&ico, at), 40, "{size}px entry is a DIB");
            assert_eq!(le32(&ico, at + 4), size, "{size}px DIB width");
            assert_eq!(le32(&ico, at + 8), size * 2, "{size}px DIB height and mask");
            let pixels = (size * size * 4) as usize;
            let mask = (size as usize).div_ceil(8).next_multiple_of(4) * size as usize;
            assert_eq!(len, 40 + pixels + mask, "{size}px DIB is fully written");
        }
        let last = 6 + 16 * (sizes.len() - 1);
        assert_eq!(
            le32(&ico, last + 12) as usize + le32(&ico, last + 8) as usize,
            ico.len(),
            "the entries cover the file exactly"
        );
    }

    #[test]
    fn dib_rows_are_bottom_up_bgra() {
        let mut pixmap = tiny_skia::Pixmap::new(2, 2).expect("pixmap");
        let rgba = [
            [1, 2, 3, 255],
            [4, 5, 6, 255],
            [7, 8, 9, 255],
            [10, 11, 12, 255],
        ];
        for (pixel, [red, green, blue, alpha]) in pixmap.pixels_mut().iter_mut().zip(rgba) {
            *pixel = tiny_skia::PremultipliedColorU8::from_rgba(red, green, blue, alpha)
                .expect("opaque test color");
        }

        let dib = dib(&pixmap);
        assert_eq!(
            &dib[40..56],
            [9, 8, 7, 255, 12, 11, 10, 255, 3, 2, 1, 255, 6, 5, 4, 255],
        );
    }

    #[test]
    fn icns_chunks_chain_to_the_declared_length() {
        let icns = icns(&tree()).expect("icns");
        assert_eq!(&icns[0..4], b"icns");
        assert_eq!(be32(&icns, 4) as usize, icns.len());

        let mut at = 8;
        for &(kind, size) in ICNS_ENTRIES {
            assert_eq!(&icns[at..at + 4], kind, "chunk order");
            let len = be32(&icns, at + 4) as usize;
            assert_eq!(&icns[at + 9..at + 12], b"PNG", "chunk carries a PNG");
            assert_eq!(be32(&icns, at + 8 + 16), size, "chunk pixel size");
            at += len;
        }
        assert_eq!(at, icns.len(), "chunks cover the file exactly");
    }

    #[test]
    fn macos_renders_keep_apple_s_margin() {
        let png = render(&tree(), 64, MACOS_SCALE).expect("render");
        let pixmap = tiny_skia::Pixmap::decode_png(&png).expect("decode");
        assert_eq!(pixmap.pixel(0, 32).expect("left edge").alpha(), 0);
        assert_eq!(pixmap.pixel(63, 32).expect("right edge").alpha(), 0);
        assert_ne!(pixmap.pixel(32, 32).expect("centre").alpha(), 0);
    }
}
