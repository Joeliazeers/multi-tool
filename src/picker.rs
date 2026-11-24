//! Dialog pilih folder/file grafis (zenity). Kalau tidak tersedia, menu memakai input teks.
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub fn available() -> bool {
    (std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some())
        && which::which("zenity").is_ok()
}

fn run(args: &[String]) -> Option<PathBuf> {
    let out = Command::new("zenity")
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None; // dibatalkan
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!s.is_empty()).then(|| PathBuf::from(s))
}

fn start_dir(p: &Path) -> String {
    let mut s = p.display().to_string();
    if !s.ends_with('/') {
        s.push('/');
    }
    s
}

pub fn folder(title: &str, start: &Path) -> Option<PathBuf> {
    run(&[
        "--file-selection".into(),
        "--directory".into(),
        format!("--title={title}"),
        format!("--filename={}", start_dir(start)),
    ])
}

pub enum Kind {
    Video,
    Srt,
}

pub fn open_file(title: &str, start: &Path, kind: Kind) -> Option<PathBuf> {
    let filter = match kind {
        Kind::Video => "Video/Audio | *.mp4 *.mkv *.webm *.mov *.avi *.flv *.ts *.m4v *.mp3 *.m4a *.wav *.ogg *.opus *.flac *.aac",
        Kind::Srt => "Subtitle SRT | *.srt *.SRT",
    };
    run(&[
        "--file-selection".into(),
        format!("--title={title}"),
        format!("--filename={}", start_dir(start)),
        format!("--file-filter={filter}"),
        "--file-filter=Semua file | *".into(),
    ])
}

pub fn save_file(title: &str, suggested: &Path) -> Option<PathBuf> {
    run(&[
        "--file-selection".into(),
        "--save".into(),
        "--confirm-overwrite".into(),
        format!("--title={title}"),
        format!("--filename={}", suggested.display()),
        "--file-filter=Subtitle SRT | *.srt".into(),
    ])
}
