use crate::config::Config;
use crate::{report, util};
use anyhow::{Context, Result, bail};
use clap::{Args, ValueEnum};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::Command;

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Quality {
    /// Kualitas terbaik yang tersedia
    #[default]
    Best,
    #[value(name = "1080p")]
    #[serde(rename = "1080p")]
    P1080,
    #[value(name = "720p")]
    #[serde(rename = "720p")]
    P720,
    #[value(name = "480p")]
    #[serde(rename = "480p")]
    P480,
    #[value(name = "360p")]
    #[serde(rename = "360p")]
    P360,
    /// Audio saja (cukup untuk transcribe)
    Audio,
}

impl Quality {
    fn format(self) -> Option<String> {
        let h = match self {
            Quality::Best => return None,
            Quality::Audio => return Some("ba/b".into()),
            Quality::P1080 => 1080,
            Quality::P720 => 720,
            Quality::P480 => 480,
            Quality::P360 => 360,
        };
        Some(format!("bv*[height<={h}]+ba/b[height<={h}]/wv*+ba/w"))
    }
}

#[derive(Args, Debug, Clone, Default, Serialize, Deserialize)]
pub struct DownloadArgs {
    /// Link video
    pub url: String,

    /// Referer (mis. halaman tempat video di-embed)
    #[arg(short = 'r', long)]
    #[serde(default)]
    pub referer: Option<String>,

    /// Kualitas video
    #[arg(short = 'q', long, value_enum, default_value_t = Quality::Best)]
    #[serde(default)]
    pub quality: Quality,

    /// Ambil cookies dari browser (firefox, chrome, brave, ...)
    #[arg(long)]
    #[serde(default)]
    pub cookies_from_browser: Option<String>,

    /// Format yt-dlp manual (menimpa --quality)
    #[arg(short = 'f', long)]
    #[serde(default)]
    pub format: Option<String>,

    /// Folder tujuan
    #[arg(short = 'o', long)]
    #[serde(default)]
    pub output_dir: Option<PathBuf>,

    /// Izinkan download seluruh playlist
    #[arg(long)]
    #[serde(default)]
    pub playlist: bool,

    /// Header HTTP tambahan, "Nama:Nilai" (boleh berulang)
    #[arg(short = 'H', long = "header")]
    #[serde(default)]
    pub headers: Vec<String>,

    /// Jumlah koneksi paralel untuk file langsung, 1-32 (default dari konfigurasi)
    #[arg(long)]
    #[serde(default)]
    pub segments: Option<usize>,

    /// Batas kecepatan download, mis. 2M atau 500K; 0 = tanpa batas (default dari konfigurasi)
    #[arg(long, value_parser = crate::fetch::limit::parse_size)]
    #[serde(default)]
    pub limit: Option<u64>,
}

impl DownloadArgs {
    /// File langsung hanya diperiksa bila user tidak memaksa format/playlist yt-dlp.
    fn may_be_direct(&self) -> bool {
        non_empty(&self.format).is_none() && !self.playlist
    }
    fn parts(&self, cfg: &Config) -> usize {
        self.segments.unwrap_or(cfg.download_segments).clamp(1, 32)
    }
}

fn non_empty(s: &Option<String>) -> Option<&str> {
    s.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

/// Opsi jaringan bersama (referer, header, cookies).
fn net_args(c: &mut Command, a: &DownloadArgs) {
    if let Some(r) = non_empty(&a.referer) {
        c.arg("--add-headers").arg(format!("Referer:{r}"));
    }
    for h in a.headers.iter().filter(|h| !h.trim().is_empty()) {
        c.arg("--add-headers").arg(h.trim());
    }
    if let Some(b) = non_empty(&a.cookies_from_browser) {
        c.arg("--cookies-from-browser").arg(b);
    }
}

const PROG: &str = "VSPROG|";

/// Download: file langsung lewat mesin multi-bagian, selain itu lewat yt-dlp. Kembalikan path hasil.
pub fn run(cfg: &Config, a: &DownloadArgs) -> Result<Vec<PathBuf>> {
    let out_dir = a.output_dir.clone().unwrap_or_else(|| cfg.output_dir());
    if a.may_be_direct()
        && let Some(info) = crate::fetch::probe(a)
    {
        if non_empty(&a.cookies_from_browser).is_some() {
            report::warn("Cookies browser hanya untuk situs video; untuk file langsung pakai header Cookie (-H).");
        }
        report::stage(format!("Download file ke {}", out_dir.display()));
        let limit = a.limit.unwrap_or(cfg.speed_limit);
        let file = crate::fetch::download(a, &info, &out_dir, a.parts(cfg), limit)?;
        report::log(format!("Tersimpan: {}", file.display()));
        return Ok(vec![file]);
    }
    util::require_bin("yt-dlp")?;
    std::fs::create_dir_all(&out_dir)
        .with_context(|| format!("gagal membuat folder {}", out_dir.display()))?;
    let list = tempfile::NamedTempFile::new()?;

    let mut c = util::command("yt-dlp");
    if cfg!(windows) {
        c.args(["--encoding", "utf-8"]); // pipa Windows default cp1252: judul non-Latin rusak
    }
    c.arg("--newline")
        .arg("-N")
        .arg(a.parts(cfg).to_string()) // fragmen HLS/DASH diunduh paralel
        .arg("--progress-template")
        .arg(format!(
            "download:{PROG}%(progress.downloaded_bytes)s|%(progress.total_bytes)s|%(progress.total_bytes_estimate)s|%(progress._speed_str)s|%(progress._eta_str)s"
        ))
        .arg("-P")
        .arg(&out_dir)
        .args(["-o", "%(title).150B [%(id)s].%(ext)s"])
        .args(["--merge-output-format", "mp4/mkv"])
        .arg("--print-to-file")
        .arg("after_move:filepath")
        .arg(list.path());
    if !a.playlist {
        c.arg("--no-playlist");
    }
    net_args(&mut c, a);
    let format = non_empty(&a.format).map(str::to_string).or_else(|| a.quality.format());
    if let Some(f) = format {
        c.arg("-f").arg(f);
    }
    c.arg("--").arg(a.url.trim());

    report::stage(format!("Download ke {}", out_dir.display()));
    let mut part = 0u32;
    util::run_streaming(&mut c, "yt-dlp", |line| {
        if let Some(rest) = line.strip_prefix(PROG) {
            let f: Vec<&str> = rest.split('|').collect();
            let num = |s: &str| s.trim().parse::<f64>().ok();
            let done = f.first().and_then(|s| num(s));
            let total = f.get(1).and_then(|s| num(s)).or_else(|| f.get(2).and_then(|s| num(s)));
            let speed = f.get(3).map(|s| s.trim()).unwrap_or("");
            let eta = f.get(4).map(|s| s.trim()).unwrap_or("");
            let fraction = match (done, total) {
                (Some(d), Some(t)) if t > 0.0 => Some(d / t),
                _ => None,
            };
            let size = total.map(|t| format!("{:.1} MB", t / 1_048_576.0)).unwrap_or_default();
            let label = if part > 1 { format!("bagian {part} · ") } else { String::new() };
            report::progress(fraction, format!("{label}{size} · {speed} · sisa {eta}"));
        } else if line.starts_with("[download] Destination:") {
            part += 1;
            report::log(line.to_string());
        } else if line.starts_with("[Merger]") {
            report::stage("Menggabungkan video & audio...");
        } else if line.starts_with("[info] Writing") {
        } else if line.starts_with("ERROR") || line.starts_with("WARNING") {
            report::warn(line.to_string());
        } else if !line.trim().is_empty() {
            report::log(line.to_string());
        }
    })?;

    let files: Vec<PathBuf> = std::fs::read_to_string(list.path())?
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(PathBuf::from)
        .filter(|p| p.exists())
        .collect();
    for f in &files {
        report::log(format!("Tersimpan: {}", f.display()));
    }
    Ok(files)
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct VideoInfo {
    pub title: String,
    pub uploader: Option<String>,
    pub duration: Option<f64>,
    pub thumbnail: Option<String>,
    pub extractor: Option<String>,
    /// Resolusi tinggi video yang tersedia, urut menurun
    pub heights: Vec<u32>,
    pub is_playlist: bool,
    pub entries: usize,
    /// "video" (lewat yt-dlp) atau "file" (file langsung, mesin multi-bagian)
    pub kind: String,
    /// Ukuran file (hanya untuk kind "file", bila diketahui)
    pub size: Option<u64>,
    pub filename: Option<String>,
    /// Server mendukung download multi-bagian dan jeda/lanjut
    pub ranges: bool,
}

/// Ambil info video tanpa download (judul, thumbnail, resolusi tersedia).
pub fn probe(a: &DownloadArgs) -> Result<VideoInfo> {
    if a.may_be_direct()
        && let Some(f) = crate::fetch::probe(a)
    {
        return Ok(VideoInfo {
            title: f.name.clone(),
            kind: "file".into(),
            size: f.size,
            filename: Some(f.name),
            ranges: f.ranges,
            ..Default::default()
        });
    }
    util::require_bin("yt-dlp")?;
    let mut c = util::command("yt-dlp");
    if cfg!(windows) {
        c.args(["--encoding", "utf-8"]); // pipa Windows default cp1252: judul non-Latin rusak
    }
    c.args(["-J", "--no-warnings", "--flat-playlist"]);
    if !a.playlist {
        c.arg("--no-playlist");
    }
    net_args(&mut c, a);
    c.arg("--").arg(a.url.trim());
    let out = c.output().context("gagal menjalankan yt-dlp")?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let msg = err.lines().rev().find(|l| l.contains("ERROR")).unwrap_or(err.trim());
        bail!("{}", msg.trim_start_matches("ERROR: ").trim());
    }
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).context("info video tidak valid")?;
    let mut heights: Vec<u32> = v["formats"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|f| f["vcodec"].as_str() != Some("none"))
        .filter_map(|f| f["height"].as_u64().map(|h| h as u32))
        .collect();
    heights.sort_unstable_by(|a, b| b.cmp(a));
    heights.dedup();
    let thumbnail = v["thumbnail"].as_str().map(str::to_string).or_else(|| {
        v["thumbnails"].as_array().and_then(|t| t.last()).and_then(|t| t["url"].as_str()).map(str::to_string)
    });
    let is_playlist = v["_type"].as_str() == Some("playlist");
    Ok(VideoInfo {
        title: v["title"].as_str().unwrap_or("(tanpa judul)").to_string(),
        uploader: v["uploader"].as_str().or(v["channel"].as_str()).map(str::to_string),
        duration: v["duration"].as_f64(),
        thumbnail,
        extractor: v["extractor_key"].as_str().map(str::to_string),
        heights,
        is_playlist,
        entries: v["entries"].as_array().map(|e| e.len()).unwrap_or(0),
        kind: "video".into(),
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fetch::testserver::{Opts, TestServer, sample};
    use clap::Parser;

    #[derive(Parser)]
    struct Wrap {
        #[command(flatten)]
        a: DownloadArgs,
    }

    #[test]
    fn cli_parses_segments_and_limit() {
        let w = Wrap::try_parse_from(["x", "https://a/b", "--segments", "4", "--limit", "2M"]).unwrap();
        assert_eq!(w.a.segments, Some(4));
        assert_eq!(w.a.limit, Some(2 * 1024 * 1024));
        let w = Wrap::try_parse_from(["x", "https://a/b"]).unwrap();
        assert_eq!((w.a.segments, w.a.limit), (None, None));
    }

    #[test]
    fn old_gui_payload_without_new_fields_still_parses() {
        let a: DownloadArgs = serde_json::from_str(r#"{"url":"https://a/b","quality":"best"}"#).unwrap();
        assert_eq!(a.segments, None);
    }

    #[test]
    fn probe_reports_direct_file() {
        let srv = TestServer::start(sample(3000), Opts::default());
        let a = DownloadArgs { url: srv.url("movie.mkv"), ..Default::default() };
        let v = probe(&a).unwrap();
        assert_eq!((v.kind.as_str(), v.size, v.ranges), ("file", Some(3000), true));
        assert_eq!(v.filename.as_deref(), Some("movie.mkv"));
    }

    #[test]
    fn run_downloads_direct_file_without_ytdlp() {
        let data = sample(3 * 1024 * 1024);
        let srv = TestServer::start(data.clone(), Opts::default());
        let dir = tempfile::tempdir().unwrap();
        let cfg = crate::config::load();
        let a = DownloadArgs {
            url: srv.url("a.bin"),
            output_dir: Some(dir.path().to_path_buf()),
            segments: Some(3),
            ..Default::default()
        };
        let files = run(&cfg, &a).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(std::fs::read(&files[0]).unwrap(), data);
    }

    #[test]
    fn manual_format_skips_direct_detection() {
        let a = DownloadArgs { url: "https://a/b".into(), format: Some("best".into()), ..Default::default() };
        assert!(!a.may_be_direct());
    }
}
