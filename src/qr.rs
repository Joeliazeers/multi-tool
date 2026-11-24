//! QR code dari teks/link. Dirender sendiri dari matriks modul `qrcode`
//! (satu jalur untuk semua format), disimpan sebagai PNG/JPG/SVG/WebP/BMP.
use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use clap::{Args, ValueEnum};
use image::codecs::jpeg::JpegEncoder;
use image::{ImageFormat, Rgb, RgbImage};
use qrcode::types::QrError;
use qrcode::{Color, EcLevel, QrCode};
use serde::Deserialize;
use std::io::Cursor;
use std::path::{Path, PathBuf};

const MAX_SIZE: u32 = 8192;
const MAX_MARGIN: u32 = 64;

/// Level koreksi error: makin tinggi makin tahan rusak, tapi QR makin padat.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, ValueEnum)]
#[serde(rename_all = "UPPERCASE")]
pub enum Ec {
    #[value(name = "L", alias = "l")]
    L,
    #[value(name = "M", alias = "m")]
    M,
    #[value(name = "Q", alias = "q")]
    Q,
    #[value(name = "H", alias = "h")]
    H,
}

#[derive(Clone, Debug, Deserialize, Args)]
#[serde(default)]
pub struct QrOptions {
    /// Ukuran gambar (piksel, sisi terpanjang). Hasil selalu kelipatan bulat piksel per modul
    #[arg(long, default_value_t = 512)]
    pub size: u32,
    /// Level koreksi error: L, M, Q, H
    #[arg(long, value_enum, default_value = "M")]
    pub ec: Ec,
    /// Warna QR (hex, mis. #000000)
    #[arg(long, default_value = "#000000")]
    pub fg: String,
    /// Warna latar (hex, mis. #ffffff)
    #[arg(long, default_value = "#ffffff")]
    pub bg: String,
    /// Lebar tepi kosong (dalam modul)
    #[arg(long, default_value_t = 4)]
    pub margin: u32,
}

impl Default for QrOptions {
    fn default() -> Self {
        Self { size: 512, ec: Ec::M, fg: "#000000".into(), bg: "#ffffff".into(), margin: 4 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Png,
    Jpg,
    Svg,
    Webp,
    Bmp,
}

const SUPPORTED: &str = "png, jpg, svg, webp, bmp";

impl Format {
    pub fn from_path(path: &Path) -> Result<Format> {
        let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        Ok(match ext.as_str() {
            "png" => Format::Png,
            "jpg" | "jpeg" => Format::Jpg,
            "svg" => Format::Svg,
            "webp" => Format::Webp,
            "bmp" => Format::Bmp,
            "" => bail!("Nama file tanpa ekstensi. Format yang didukung: {SUPPORTED}"),
            other => bail!("Format .{other} tidak didukung. Format yang didukung: {SUPPORTED}"),
        })
    }
}

fn parse_color(s: &str) -> Result<[u8; 3]> {
    let t = s.trim();
    let hex = t.strip_prefix('#').unwrap_or(t);
    let bad = || anyhow!("Warna '{s}' tidak valid. Pakai hex, mis. #000000 atau #fff");
    if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(bad());
    }
    let nib = |i: usize| u8::from_str_radix(&hex[i..=i], 16).unwrap();
    let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).unwrap();
    match hex.len() {
        3 => Ok([nib(0) * 17, nib(1) * 17, nib(2) * 17]),
        6 => Ok([byte(0), byte(2), byte(4)]),
        _ => Err(bad()),
    }
}

/// Matriks modul + ukuran kanvas dalam modul (termasuk margin) dan skala piksel per modul.
struct Layout {
    modules: usize,
    margin: usize,
    scale: u32,
    dark: Vec<bool>,
    fg: [u8; 3],
    bg: [u8; 3],
}

impl Layout {
    fn total(&self) -> u32 {
        (self.modules + 2 * self.margin) as u32
    }
    fn side(&self) -> u32 {
        self.total() * self.scale
    }
}

fn layout(text: &str, o: &QrOptions) -> Result<Layout> {
    let text = text.trim();
    if text.is_empty() {
        bail!("Teks kosong: isi link atau teks dulu");
    }
    if o.size > MAX_SIZE {
        bail!("Ukuran maksimal {MAX_SIZE} piksel");
    }
    let (fg, bg) = (parse_color(&o.fg)?, parse_color(&o.bg)?);
    if fg == bg {
        bail!("Warna QR dan latar sama, QR tidak akan terbaca");
    }
    let level = match o.ec {
        Ec::L => EcLevel::L,
        Ec::M => EcLevel::M,
        Ec::Q => EcLevel::Q,
        Ec::H => EcLevel::H,
    };
    let code = QrCode::with_error_correction_level(text.as_bytes(), level).map_err(|e| match e {
        QrError::DataTooLong => anyhow!("Teks terlalu panjang untuk QR code. Persingkat teks atau turunkan level koreksi error"),
        e => anyhow!("Gagal membuat QR code: {e}"),
    })?;
    let modules = code.width();
    let margin = o.margin.min(MAX_MARGIN) as usize;
    let total = (modules + 2 * margin) as u32;
    Ok(Layout {
        modules,
        margin,
        scale: (o.size / total).max(1),
        dark: code.to_colors().into_iter().map(|c| c == Color::Dark).collect(),
        fg,
        bg,
    })
}

fn raster(l: &Layout) -> RgbImage {
    let mut img = RgbImage::from_pixel(l.side(), l.side(), Rgb(l.bg));
    for (i, _) in l.dark.iter().enumerate().filter(|(_, d)| **d) {
        let (mx, my) = ((i % l.modules + l.margin) as u32, (i / l.modules + l.margin) as u32);
        for dy in 0..l.scale {
            for dx in 0..l.scale {
                img.put_pixel(mx * l.scale + dx, my * l.scale + dy, Rgb(l.fg));
            }
        }
    }
    img
}

fn svg(l: &Layout) -> String {
    let hex = |c: [u8; 3]| format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2]);
    let mut path = String::new();
    for (i, _) in l.dark.iter().enumerate().filter(|(_, d)| **d) {
        let (x, y) = (i % l.modules + l.margin, i / l.modules + l.margin);
        path.push_str(&format!("M{x} {y}h1v1h-1z"));
    }
    let (t, side) = (l.total(), l.side());
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {t} {t}\" width=\"{side}\" height=\"{side}\" shape-rendering=\"crispEdges\">\n<rect width=\"{t}\" height=\"{t}\" fill=\"{bg}\"/>\n<path fill=\"{fg}\" d=\"{path}\"/>\n</svg>\n",
        bg = hex(l.bg),
        fg = hex(l.fg),
    )
}

/// Buat QR dari `text` dan kodekan ke `fmt`. Sisi gambar = (modul + 2·margin) × skala bulat,
/// jadi bisa sedikit di bawah `opts.size`, tidak pernah di atasnya (kecuali 1 px/modul).
pub fn encode(text: &str, opts: &QrOptions, fmt: Format) -> Result<Vec<u8>> {
    let l = layout(text, opts)?;
    if fmt == Format::Svg {
        return Ok(svg(&l).into_bytes());
    }
    let img = raster(&l);
    let mut out = Cursor::new(Vec::new());
    match fmt {
        Format::Jpg => img.write_with_encoder(JpegEncoder::new_with_quality(&mut out, 95))?,
        Format::Png => img.write_to(&mut out, ImageFormat::Png)?,
        Format::Webp => img.write_to(&mut out, ImageFormat::WebP)?,
        Format::Bmp => img.write_to(&mut out, ImageFormat::Bmp)?,
        Format::Svg => unreachable!(),
    }
    Ok(out.into_inner())
}

pub fn png_data_url(text: &str, opts: &QrOptions) -> Result<String> {
    let png = encode(text, opts, Format::Png)?;
    Ok(format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(png)))
}

/// Simpan ke `path`; format dari ekstensi. Tanpa ekstensi ditambah `.png`. Mengembalikan path akhir.
pub fn save(text: &str, opts: &QrOptions, path: &Path) -> Result<PathBuf> {
    let mut path = path.to_path_buf();
    if path.extension().is_none() {
        path.set_extension("png");
    }
    let fmt = Format::from_path(&path)?;
    let bytes = encode(text, opts, fmt)?;
    std::fs::write(&path, bytes).with_context(|| format!("Gagal menyimpan {}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use qrcode::{EcLevel, QrCode};

    const URL: &str = "https://example.com";
    const PNG_SIG: [u8; 4] = [0x89, b'P', b'N', b'G'];

    fn modules(text: &str, level: EcLevel) -> u32 {
        QrCode::with_error_correction_level(text.as_bytes(), level).unwrap().width() as u32
    }

    fn opts(f: impl FnOnce(&mut QrOptions)) -> QrOptions {
        let mut o = QrOptions::default();
        f(&mut o);
        o
    }

    fn decode(bytes: &[u8]) -> image::RgbImage {
        image::load_from_memory(bytes).unwrap().to_rgb8()
    }

    #[test]
    fn png_has_signature_and_square_size() {
        let bytes = encode(URL, &Default::default(), Format::Png).unwrap();
        assert_eq!(bytes[..4], PNG_SIG);
        let img = decode(&bytes);
        assert_eq!(img.width(), img.height());
        assert!((256..=512).contains(&img.width()), "width {}", img.width());
    }

    #[test]
    fn size_is_whole_pixels_per_module() {
        let total = modules(URL, EcLevel::M) + 8;
        let img = decode(&encode(URL, &opts(|o| o.size = 100), Format::Png).unwrap());
        assert_eq!(img.width() % total, 0);
        assert!(img.width() <= 100 && img.width() >= total, "width {}", img.width());
    }

    #[test]
    fn tiny_size_falls_back_to_one_px_per_module() {
        let total = modules(URL, EcLevel::M) + 8;
        let img = decode(&encode(URL, &opts(|o| o.size = 10), Format::Png).unwrap());
        assert_eq!(img.width(), total);
    }

    #[test]
    fn svg_contains_colors() {
        let bytes = encode(URL, &opts(|o| o.fg = "#ff0000".into()), Format::Svg).unwrap();
        let svg = String::from_utf8(bytes).unwrap();
        assert!(svg.contains("<svg"));
        assert!(svg.contains("#ff0000"));
    }

    #[test]
    fn jpg_webp_bmp_decode() {
        for fmt in [Format::Jpg, Format::Webp, Format::Bmp] {
            let bytes = encode(URL, &Default::default(), fmt).unwrap();
            assert!(image::load_from_memory(&bytes).is_ok(), "{fmt:?}");
        }
    }

    #[test]
    fn top_left_is_background_and_has_dark_modules() {
        let o = opts(|o| {
            o.fg = "#112233".into();
            o.bg = "#ddeeff".into();
        });
        let img = decode(&encode(URL, &o, Format::Png).unwrap());
        assert_eq!(img.get_pixel(0, 0).0, [0xdd, 0xee, 0xff]);
        assert!(img.pixels().any(|p| p.0 == [0x11, 0x22, 0x33]));
    }

    #[test]
    fn higher_ec_makes_bigger_or_equal_code() {
        let text = "x".repeat(200);
        let w = |ec| decode(&encode(&text, &opts(|o| { o.size = 1; o.ec = ec; }), Format::Png).unwrap()).width();
        assert!(w(Ec::H) >= w(Ec::L));
    }

    #[test]
    fn empty_text_errors() {
        for t in ["", "   "] {
            let e = encode(t, &Default::default(), Format::Png).unwrap_err().to_string();
            assert!(e.contains("kosong"), "{e}");
        }
    }

    #[test]
    fn too_long_text_errors() {
        let e = encode(&"a".repeat(8000), &Default::default(), Format::Png).unwrap_err().to_string();
        assert!(e.contains("terlalu panjang"), "{e}");
    }

    #[test]
    fn oversize_size_errors_instead_of_allocating() {
        let e = encode(URL, &opts(|o| o.size = 4_000_000_000), Format::Png).unwrap_err().to_string();
        assert!(e.contains("maksimal"), "{e}");
    }

    #[test]
    fn bad_colors_error() {
        for bad in ["red", "#12", "#gggggg", "##000"] {
            assert!(encode(URL, &opts(|o| o.fg = bad.into()), Format::Png).is_err(), "{bad}");
        }
        for good in ["#fff", "#FFFFFF"] {
            assert!(encode(URL, &opts(|o| { o.fg = "#000".into(); o.bg = good.into(); }), Format::Png).is_ok(), "{good}");
        }
        let e = encode(URL, &opts(|o| { o.fg = "#abc".into(); o.bg = "#aabbcc".into(); }), Format::Png).unwrap_err().to_string();
        assert!(e.contains("sama"), "{e}");
    }

    #[test]
    fn format_from_path() {
        use std::path::Path;
        let f = |p: &str| Format::from_path(Path::new(p)).unwrap();
        assert_eq!(f("a.PNG"), Format::Png);
        assert_eq!(f("a.jpeg"), Format::Jpg);
        assert_eq!(f("a.jpg"), Format::Jpg);
        assert_eq!(f("a.svg"), Format::Svg);
        assert_eq!(f("a.webp"), Format::Webp);
        assert_eq!(f("a.bmp"), Format::Bmp);
        for bad in ["a.txt", "a"] {
            let e = Format::from_path(Path::new(bad)).unwrap_err().to_string();
            assert!(e.contains("png, jpg, svg, webp, bmp"), "{e}");
        }
    }

    #[test]
    fn save_appends_png_when_no_extension() {
        let dir = tempfile::tempdir().unwrap();
        let p = save(URL, &Default::default(), &dir.path().join("qr")).unwrap();
        assert_eq!(p, dir.path().join("qr.png"));
        assert_eq!(std::fs::read(&p).unwrap()[..4], PNG_SIG);

        let s = save(URL, &Default::default(), &dir.path().join("x.svg")).unwrap();
        let text = std::fs::read_to_string(s).unwrap();
        assert!(text.starts_with("<?xml") || text.starts_with("<svg"));
    }

    #[test]
    fn data_url_prefix() {
            let url = png_data_url(URL, &Default::default()).unwrap();
        let b64 = url.strip_prefix("data:image/png;base64,").expect("prefix");
        let bytes = base64::engine::general_purpose::STANDARD.decode(b64).unwrap();
        assert_eq!(bytes[..4], PNG_SIG);
    }
}
