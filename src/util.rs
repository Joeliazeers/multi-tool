use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Bersihkan path hasil ketik / drag-and-drop: kutip, file://, ~.
pub fn clean_path(s: &str) -> PathBuf {
    let mut s = s.trim();
    for q in ['\'', '"'] {
        if s.len() >= 2 && s.starts_with(q) && s.ends_with(q) {
            s = &s[1..s.len() - 1];
        }
    }
    let s = s.strip_prefix("file://").unwrap_or(s);
    // file:///C:/dir/x -> C:/dir/x (huruf drive Windows)
    let s = match s.as_bytes() {
        [b'/', d, b':', b'/' | b'\\', ..] if d.is_ascii_alphabetic() => &s[1..],
        _ => s,
    };
    let mut s = s.replace("%20", " ");
    if !cfg!(windows) {
        s = s.replace("\\ ", " "); // "My\ Video" hasil seret ke terminal; di Windows `\` adalah pemisah folder
    }
    let home_rest = s.strip_prefix("~/").or_else(|| cfg!(windows).then(|| s.strip_prefix("~\\")).flatten());
    if let Some(rest) = home_rest
        && let Some(home) = dirs::home_dir()
    {
        return home.join(rest);
    }
    PathBuf::from(s)
}

/// `Command::new` yang tidak memunculkan jendela konsol di Windows (GUI memanggil yt-dlp/ffmpeg/python).
pub fn command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    #[allow(unused_mut)]
    let mut c = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    c
}

pub fn require_bin(name: &str) -> Result<PathBuf> {
    which::which(name).with_context(|| format!("`{name}` tidak ditemukan di PATH. Install dulu."))
}

pub fn run(cmd: &mut Command, what: &str) -> Result<()> {
    let status = cmd
        .status()
        .with_context(|| format!("gagal menjalankan {what}"))?;
    if !status.success() {
        bail!("{what} gagal ({status})");
    }
    Ok(())
}

/// "video.ja.srt" -> "video"; "video.mp4" -> "video"
pub fn base_stem(p: &Path) -> String {
    let stem = p
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "output".into());
    if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("srt"))
        && let Some((base, suffix)) = stem.rsplit_once('.')
        && (2..=3).contains(&suffix.len())
        && suffix.chars().all(|c| c.is_ascii_lowercase())
    {
        return base.to_string();
    }
    stem
}

/// Kode bahasa dari nama file "video.ja.srt" -> Some("ja")
pub fn lang_suffix(p: &Path) -> Option<String> {
    let stem = p.file_stem()?.to_string_lossy().into_owned();
    let (_, suffix) = stem.rsplit_once('.')?;
    ((2..=3).contains(&suffix.len()) && suffix.chars().all(|c| c.is_ascii_lowercase()))
        .then(|| suffix.to_string())
}


fn bigrams(s: &str) -> Vec<(char, char)> {
    let chars: Vec<char> = s
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect();
    chars.windows(2).map(|w| (w[0], w[1])).collect()
}

/// Dice coefficient bigram karakter (0..1), bekerja untuk semua aksara.
/// None jika teks terlalu pendek untuk dibandingkan.
pub fn dice(a: &str, b: &str) -> Option<f64> {
    let a = bigrams(a);
    let b = bigrams(b);
    if a.len() < 4 || b.len() < 4 {
        return None;
    }
    let mut pool = b.clone();
    let mut hits = 0usize;
    for g in &a {
        if let Some(i) = pool.iter().position(|x| x == g) {
            pool.swap_remove(i);
            hits += 1;
        }
    }
    Some(2.0 * hits as f64 / (a.len() + b.len()) as f64)
}

/// Untuk koreksi dalam bahasa yang sama: teks baru masih "baris yang sama"?
pub fn similar_enough(orig: &str, new: &str) -> bool {
    let (la, lb) = (bigrams(orig).len(), bigrams(new).len());
    let Some(d) = dice(orig, new) else {
        // Terlalu pendek untuk dibandingkan: cukup pastikan tidak "tumbuh" jauh lebih panjang
        // (mis. "过来" jadi satu kalimat utuh yang diambil dari baris lain).
        let n = |t: &str| t.chars().filter(|c| c.is_alphanumeric()).count();
        return n(new) <= n(orig) * 2 + 4;
    };
    let ratio = lb as f64 / la as f64;
    (0.5..=1.8).contains(&ratio) && d >= 0.45
}

/// Tentukan path output. `out` boleh berupa folder (file default dibuat di dalamnya)
/// atau path file `.srt`. None = `default_dir/default_name`. Folder dibuat bila belum ada.
pub fn resolve_out(out: Option<&Path>, default_dir: &Path, default_name: &str) -> anyhow::Result<PathBuf> {
    let path = match out {
        None => default_dir.join(default_name),
        Some(p) => {
            let is_file = !p.is_dir()
                && p.extension().is_some_and(|e| e.eq_ignore_ascii_case("srt"));
            if is_file { p.to_path_buf() } else { p.join(default_name) }
        }
    };
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("gagal membuat folder {}", parent.display()))?;
    }
    Ok(path)
}

/// Jalankan proses, teruskan setiap baris stdout/stderr (dipisah \n atau \r) ke `on_line`,
/// dan hentikan proses kalau user membatalkan. Error menyertakan beberapa baris terakhir.
pub fn run_streaming(cmd: &mut Command, what: &str, mut on_line: impl FnMut(&str)) -> Result<()> {
    use std::io::Read;
    use std::process::Stdio;
    use std::sync::mpsc;
    use std::time::Duration;

    crate::report::check()?;
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("gagal menjalankan {what}"))?;

    let (tx, rx) = mpsc::channel::<String>();
    let mut readers = Vec::new();
    for stream in [
        child.stdout.take().map(|s| Box::new(s) as Box<dyn Read + Send>),
        child.stderr.take().map(|s| Box::new(s) as Box<dyn Read + Send>),
    ]
    .into_iter()
    .flatten()
    {
        let tx = tx.clone();
        readers.push(std::thread::spawn(move || {
            let mut stream = stream;
            let mut buf = [0u8; 4096];
            let mut pending: Vec<u8> = Vec::new();
            while let Ok(n) = stream.read(&mut buf) {
                if n == 0 {
                    break;
                }
                for &b in &buf[..n] {
                    if b == b'\n' || b == b'\r' {
                        if !pending.is_empty() {
                            let _ = tx.send(String::from_utf8_lossy(&pending).into_owned());
                            pending.clear();
                        }
                    } else {
                        pending.push(b);
                    }
                }
            }
            if !pending.is_empty() {
                let _ = tx.send(String::from_utf8_lossy(&pending).into_owned());
            }
        }));
    }
    drop(tx);

    let mut tail: std::collections::VecDeque<String> = std::collections::VecDeque::new();
    let status = loop {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(line) => {
                on_line(&line);
                tail.push_back(line);
                if tail.len() > 12 {
                    tail.pop_front();
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break child.wait()?,
        }
        if crate::report::cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            bail!("Dibatalkan");
        }
    };
    for r in readers {
        let _ = r.join();
    }
    if !status.success() {
        let last: Vec<String> = tail.into_iter().rev().take(6).collect::<Vec<_>>().into_iter().rev().collect();
        bail!("{what} gagal ({status})\n{}", last.join("\n"));
    }
    Ok(())
}

/// Token untuk mendeteksi kata pinjaman antar baris: kata (≥4 huruf) untuk bahasa berspasi,
/// bigram karakter untuk bahasa tanpa spasi (Jepang, Mandarin, Thai).
fn tokens(s: &str, words: bool) -> Vec<String> {
    let lower = s.to_lowercase();
    if words {
        lower
            .split(|c: char| !c.is_alphanumeric() && c != '\'')
            .filter(|w| w.chars().count() >= 4)
            .map(str::to_string)
            .collect()
    } else {
        let chars: Vec<char> = lower.chars().filter(|c| c.is_alphanumeric()).collect();
        chars.windows(2).map(|w| w.iter().collect()).collect()
    }
}

/// True kalau `new` menambahkan kata yang tidak ada di `orig` tapi ada di baris sekitar
/// (`neighbors`, mis. ±2 baris) — tanda LLM memindahkan/menyalin isi baris lain sehingga
/// timing tidak sinkron.
pub fn borrows_from_neighbors(orig: &str, new: &str, neighbors: &[&str]) -> bool {
    // Satu mode untuk semua teks yang dibandingkan (baris "scientists," tanpa spasi tetap kata).
    let words = [orig, new].iter().chain(neighbors).any(|t| t.trim().contains(' '));
    let own: std::collections::HashSet<String> = tokens(orig, words).into_iter().collect();
    let neigh: std::collections::HashSet<String> =
        neighbors.iter().flat_map(|t| tokens(t, words)).filter(|t| !own.contains(t)).collect();
    tokens(new, words).iter().any(|t| !own.contains(t) && neigh.contains(t))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn out_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let t = tmp.path();
        // default
        assert_eq!(resolve_out(None, t, "a.id.srt").unwrap(), t.join("a.id.srt"));
        // folder yang sudah ada
        assert_eq!(resolve_out(Some(t), t, "a.id.srt").unwrap(), t.join("a.id.srt"));
        // folder baru (tanpa .srt) dibuat otomatis
        let p = resolve_out(Some(&t.join("baru/sub")), t, "a.srt").unwrap();
        assert_eq!(p, t.join("baru/sub/a.srt"));
        assert!(t.join("baru/sub").is_dir());
        // file .srt eksplisit, parent dibuat
        let f = t.join("x/nama-saya.srt");
        assert_eq!(resolve_out(Some(&f), t, "a.srt").unwrap(), f);
        assert!(t.join("x").is_dir());
    }

    #[test]
    fn neighbor_borrowing() {
        // Kasus nyata dari Aya 8B saat merapikan transkrip JFK.
        let l = [
            "Senator Wiley and Congressman Miller, Mr. Webb, Mr. Bell,",
            "scientists,",
            "I am delighted to be here, and I'm particularly delighted to be here on this",
            "occasion.",
            "The greater our knowledge increases, the greater our ignorance unfolds.",
            "despite the striking fact that most of the scientists that the world has ever known",
            "are alive and working today despite the fact that this nation's own",
        ];
        assert!(borrows_from_neighbors(l[0], "Senator Wiley and Congressman Miller, Mr. Webb, Mr. Bell, scientists,", &[l[1]]));
        assert!(borrows_from_neighbors(l[2], "I am delighted to be here, and I'm particularly pleased to be here on this occasion.", &[l[3]]));
        assert!(borrows_from_neighbors(l[5], "are alive and working today, despite the fact that this nation's own", &[l[4], l[6]]));
        // Koreksi wajar tidak ditolak
        assert!(!borrows_from_neighbors("President Pitta, Mr. Vice President,", "President Pitzer, Mr. Vice President,", &[l[0]]));
        assert!(!borrows_from_neighbors(l[6], "are alive and working today, despite the fact that this nation's own", &[l[5]]));
        // Bahasa tanpa spasi
        assert!(borrows_from_neighbors("今日はいい天気", "今日はいい天気ですね散歩", &["散歩しましょう"]));
    }

    #[test]
    fn short_line_and_two_away() {
        // Kasus nyata (Aya 8B, transkrip Mandarin): baris 4 "过来" diisi kalimat dari baris 5-6.
        let (b3, b4, b5, b6) = ("你不要", "过来", "给", "姐姐看一下你的鸡鸡你是处男吗");
        let bad = "过来，给姐姐看一下你的鸡鸡，你是处男吗？";
        assert!(!similar_enough(b4, bad));
        assert!(borrows_from_neighbors(b4, bad, &[b3, b5, b6]));
        // Koreksi wajar pada baris pendek tetap diterima
        assert!(similar_enough(b4, "过来。"));
        assert!(similar_enough("Uh", "Uh-huh."));
    }

    #[test]
    fn stems() {
        assert_eq!(base_stem(Path::new("/v/film.ja.srt")), "film");
        assert_eq!(base_stem(Path::new("/v/film.mp4")), "film");
        assert_eq!(lang_suffix(Path::new("film.ja.srt")).as_deref(), Some("ja"));
    }
}


#[cfg(test)]
mod path_tests {
    use super::clean_path;
    use std::path::PathBuf;

    #[test]
    fn file_url_with_windows_drive_letter() {
        assert_eq!(clean_path("file:///C:/Users/a/My%20Video.mp4"), PathBuf::from("C:/Users/a/My Video.mp4"));
    }

    #[test]
    fn file_url_unix_keeps_root() {
        assert_eq!(clean_path("file:///home/a/v.mp4"), PathBuf::from("/home/a/v.mp4"));
    }

    #[test]
    fn quoted_windows_path_keeps_backslashes() {
        assert_eq!(clean_path(r#""C:\Users\a b\v.mp4""#), PathBuf::from(r"C:\Users\a b\v.mp4"));
    }
}
