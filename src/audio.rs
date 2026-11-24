use crate::util;
use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;

const FILTER: &str = "highpass=f=80,dynaudnorm=f=150:g=15:p=0.9,loudnorm=I=-16";

fn ffmpeg() -> Command {
    let mut c = util::command("ffmpeg");
    c.args(["-hide_banner", "-nostdin", "-v", "error", "-y"]);
    c
}

/// Ekstrak audio mono 16 kHz WAV (format ideal untuk Whisper).
pub fn extract_wav(input: &Path, out: &Path, filter: bool) -> Result<()> {
    util::require_bin("ffmpeg")?;
    let mut c = ffmpeg();
    c.arg("-i").arg(input).args(["-vn", "-ac", "1", "-ar", "16000"]);
    if filter {
        c.args(["-af", FILTER]);
    }
    c.arg(out);
    util::run_streaming(&mut c, "ffmpeg (ekstrak audio)", |_| {})
}

pub fn duration(path: &Path) -> Result<f64> {
    util::require_bin("ffprobe")?;
    let out = util::command("ffprobe")
        .args(["-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0"])
        .arg(path)
        .output()
        .context("gagal menjalankan ffprobe")?;
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse::<f64>()
        .context("durasi audio tidak terbaca (file bukan video/audio?)")
}

/// Potong bagian audio. `mp3 = true` untuk upload ke API (ukuran kecil).
pub fn cut(input: &Path, out: &Path, start: f64, len: f64, mp3: bool) -> Result<()> {
    let mut c = ffmpeg();
    c.args(["-ss", &format!("{start:.3}"), "-t", &format!("{len:.3}")])
        .arg("-i")
        .arg(input)
        .args(["-vn", "-ac", "1", "-ar", "16000"]);
    if mp3 {
        c.args(["-c:a", "libmp3lame", "-b:a", "48k"]);
    }
    c.arg(out);
    util::run_streaming(&mut c, "ffmpeg (potong audio)", |_| {})
}
