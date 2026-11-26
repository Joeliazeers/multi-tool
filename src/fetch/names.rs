//! Nama file dari header/URL, sanitasi, dan nama unik.
use std::path::{Path, PathBuf};

const RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9", "LPT1",
    "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let hex = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2]))
        {
            out.push(h * 16 + l);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Bersihkan nama: hanya komponen terakhir, tanpa karakter terlarang, bukan nama khusus Windows.
pub fn sanitize(name: &str) -> String {
    let last = name.rsplit(['/', '\\']).next().unwrap_or("");
    let cleaned: String = last
        .chars()
        .map(|c| if c.is_control() || "<>:\"|?*".contains(c) { '_' } else { c })
        .collect();
    let cleaned = cleaned.trim_matches(|c: char| c == '.' || c.is_whitespace()).to_string();
    if cleaned.is_empty() {
        return "download".into();
    }
    let stem = cleaned.split('.').next().unwrap_or("").to_ascii_uppercase();
    let mut out = if RESERVED.contains(&stem.as_str()) { format!("_{cleaned}") } else { cleaned };
    if out.len() > 200 {
        // potong aman di batas karakter
        let mut cut = 200;
        while !out.is_char_boundary(cut) {
            cut -= 1;
        }
        out.truncate(cut);
    }
    out
}

fn from_disposition(cd: &str) -> Option<String> {
    let lower = cd.to_ascii_lowercase();
    if let Some(p) = lower.find("filename*=") {
        let v = cd[p + 10..].split(';').next()?.trim().trim_matches('"');
        let v = v.splitn(3, '\'').last().unwrap_or(v);
        let d = percent_decode(v);
        if !d.trim().is_empty() {
            return Some(d);
        }
    }
    // cari "filename=" yang bukan bagian dari "filename*="
    let mut from = 0;
    while let Some(p) = lower[from..].find("filename=") {
        let at = from + p;
        let prev_ok = at == 0 || !lower.as_bytes()[at - 1].is_ascii_alphanumeric();
        if prev_ok {
            let v = cd[at + 9..].split(';').next()?.trim().trim_matches('"');
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
        from = at + 9;
    }
    None
}

/// Nama dari `Content-Disposition`, kalau tak ada dari segmen terakhir URL. Tak pernah kosong.
pub fn from_response(content_disposition: Option<&str>, url: &str) -> String {
    if let Some(n) = content_disposition.and_then(from_disposition) {
        return sanitize(&n);
    }
    let path = url.split(['?', '#']).next().unwrap_or("");
    let path = path.split_once("://").map(|(_, r)| r).unwrap_or(path);
    let path = path.split_once('/').map(|(_, p)| p).unwrap_or("");
    let seg = path.rsplit('/').next().unwrap_or("");
    sanitize(&percent_decode(seg))
}

/// `dir/name`, atau `dir/name (1)` dst. bila nama itu (atau `.part`-nya) sudah ada.
pub fn unique(dir: &Path, name: &str) -> PathBuf {
    let taken = |n: &str| dir.join(n).exists() || dir.join(format!("{n}.part")).exists();
    if !taken(name) {
        return dir.join(name);
    }
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    (1..)
        .map(|i| format!("{stem} ({i}){ext}"))
        .find(|n| !taken(n))
        .map(|n| dir.join(n))
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disposition_variants() {
        assert_eq!(from_response(Some("attachment; filename*=UTF-8''na%20ma.zip"), "https://x/y"), "na ma.zip");
        assert_eq!(from_response(Some("attachment; filename=\"a.zip\""), "https://x/y"), "a.zip");
        assert_eq!(
            from_response(Some("attachment; filename=\"fallback.zip\"; filename*=UTF-8''asli.zip"), "https://x/y"),
            "asli.zip"
        );
    }

    #[test]
    fn url_fallback() {
        assert_eq!(from_response(None, "https://x/y/file%20(1).iso?token=1"), "file (1).iso");
        assert_eq!(from_response(None, "https://x/"), "download");
        assert_eq!(from_response(None, "https://x/a%é.zip"), "a%é.zip");
        assert_eq!(from_response(None, "https://x/a%41"), "aA");
        assert_eq!(from_response(None, "https://x"), "download");
    }

    #[test]
    fn sanitize_hostile() {
        assert_eq!(sanitize("../../etc/passwd"), "passwd");
        assert_eq!(sanitize("a\\b:c*?.txt"), "b_c__.txt");
        assert_eq!(sanitize("x:y*z.txt"), "x_y_z.txt");
        assert_eq!(sanitize("CON.txt"), "_CON.txt");
        assert_eq!(sanitize("name. "), "name");
        assert_eq!(sanitize(""), "download");
        assert_eq!(sanitize(".."), "download");
        assert_eq!(sanitize("a\0b"), "a_b");
    }

    #[test]
    fn hostile_disposition_stays_inside() {
        assert_eq!(from_response(Some("attachment; filename=\"../../x.sh\""), "https://x/"), "x.sh");
    }

    #[test]
    fn unique_names() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(unique(d.path(), "a.zip"), d.path().join("a.zip"));
        std::fs::write(d.path().join("a.zip"), b"x").unwrap();
        assert_eq!(unique(d.path(), "a.zip"), d.path().join("a (1).zip"));
        std::fs::write(d.path().join("a (1).zip.part"), b"x").unwrap();
        assert_eq!(unique(d.path(), "a.zip"), d.path().join("a (2).zip"));
        std::fs::write(d.path().join("noext"), b"x").unwrap();
        assert_eq!(unique(d.path(), "noext"), d.path().join("noext (1)"));
    }
}
