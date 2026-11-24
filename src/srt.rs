use anyhow::{Result, bail};

#[derive(Clone, Debug)]
pub struct Cue {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

pub fn fmt_ts(t: f64) -> String {
    let ms_total = (t.max(0.0) * 1000.0).round() as u64;
    let h = ms_total / 3_600_000;
    let m = (ms_total / 60_000) % 60;
    let s = (ms_total / 1000) % 60;
    let ms = ms_total % 1000;
    format!("{h:02}:{m:02}:{s:02},{ms:03}")
}

pub fn parse_ts(s: &str) -> Option<f64> {
    let s = s.trim().replace('.', ",");
    let (hms, frac) = match s.split_once(',') {
        Some((a, b)) => (a.to_string(), b.to_string()),
        None => (s.clone(), "0".to_string()),
    };
    let nums: Vec<u64> = hms
        .split(':')
        .map(|p| p.trim().parse::<u64>())
        .collect::<Result<_, _>>()
        .ok()?;
    let (h, m, sec) = match nums.as_slice() {
        [h, m, s] => (*h, *m, *s),
        [m, s] => (0, *m, *s),
        _ => return None,
    };
    let mut frac: String = frac.chars().filter(|c| c.is_ascii_digit()).take(3).collect();
    while frac.len() < 3 {
        frac.push('0');
    }
    let ms: u64 = frac.parse().ok()?;
    Some((h * 3600 + m * 60 + sec) as f64 + ms as f64 / 1000.0)
}

fn is_index(line: &str) -> bool {
    let t = line.trim();
    !t.is_empty() && t.chars().all(|c| c.is_ascii_digit())
}

pub fn parse(content: &str) -> Result<Vec<Cue>> {
    let content = content
        .trim_start_matches('\u{feff}')
        .replace("\r\n", "\n")
        .replace('\r', "\n");
    let lines: Vec<&str> = content.lines().collect();
    let mut cues = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let Some((a, b)) = lines[i].split_once("-->") else {
            i += 1;
            continue;
        };
        let start = parse_ts(a);
        let end = parse_ts(b.split_whitespace().next().unwrap_or(""));
        i += 1;
        let mut text = Vec::new();
        while i < lines.len() && !lines[i].trim().is_empty() {
            if lines[i].contains("-->") {
                break;
            }
            if is_index(lines[i]) && i + 1 < lines.len() && lines[i + 1].contains("-->") {
                break;
            }
            text.push(lines[i].trim_end());
            i += 1;
        }
        if let (Some(start), Some(end)) = (start, end) {
            cues.push(Cue { start, end, text: text.join("\n") });
        }
    }
    if cues.is_empty() {
        bail!("Tidak ada subtitle valid di file SRT");
    }
    Ok(cues)
}

pub fn render(cues: &[Cue]) -> String {
    let mut out = String::new();
    let mut n = 0;
    for c in cues.iter().filter(|c| !c.text.trim().is_empty()) {
        n += 1;
        out.push_str(&format!(
            "{n}\n{} --> {}\n{}\n\n",
            fmt_ts(c.start),
            fmt_ts(c.end),
            c.text.trim()
        ));
    }
    out
}

/// Pecah baris panjang (bahasa berspasi) jadi maks ~2 baris yang seimbang.
pub fn wrap(text: &str, max: usize) -> String {
    let text = text.trim();
    if text.contains('\n') || text.chars().count() <= max || !text.contains(' ') {
        return text.to_string();
    }
    let words: Vec<&str> = text.split_whitespace().collect();
    let total = text.chars().count();
    if total <= max * 2 {
        // cari titik potong paling dekat tengah
        let mut best = (usize::MAX, 1);
        let mut len = 0;
        for (k, w) in words.iter().enumerate().take(words.len() - 1) {
            len += w.chars().count() + if k > 0 { 1 } else { 0 };
            let diff = (len as isize - (total as isize - len as isize - 1)).unsigned_abs();
            if diff < best.0 {
                best = (diff, k + 1);
            }
        }
        return format!("{}\n{}", words[..best.1].join(" "), words[best.1..].join(" "));
    }
    let mut lines = vec![String::new()];
    for w in words {
        let cur = lines.last_mut().unwrap();
        if !cur.is_empty() && cur.chars().count() + 1 + w.chars().count() > max {
            lines.push(w.to_string());
        } else {
            if !cur.is_empty() {
                cur.push(' ');
            }
            cur.push_str(w);
        }
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let src = "\u{feff}1\r\n00:00:01,500 --> 00:00:03,000\r\nHalo\r\ndunia\r\n\r\n2\r\n00:01:02.25 --> 00:01:04,000 X1:0\r\nLagi\r\n";
        let cues = parse(src).unwrap();
        assert_eq!(cues.len(), 2);
        assert_eq!(cues[0].text, "Halo\ndunia");
        assert!((cues[1].start - 62.25).abs() < 1e-9);
        let out = render(&cues);
        assert!(out.starts_with("1\n00:00:01,500 --> 00:00:03,000\nHalo\ndunia\n\n2\n00:01:02,250"));
    }

    #[test]
    fn ts_format() {
        assert_eq!(fmt_ts(3725.0456), "01:02:05,046");
    }

    #[test]
    fn wrap_balanced() {
        let w = wrap("ini adalah kalimat yang cukup panjang untuk dipecah jadi dua", 42);
        assert_eq!(w.lines().count(), 2);
        assert_eq!(wrap("pendek", 42), "pendek");
    }
}
