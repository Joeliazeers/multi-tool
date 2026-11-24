use crate::audio;
use crate::config::{AsrBackend, Config};
use crate::{report, util};
use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use serde_json::Value;
use std::path::Path;
use std::time::Duration;

const LOCAL_SCRIPT: &str = include_str!("whisper_local.py");
/// Panjang potongan audio untuk API (batas upload biasanya 25 MB).
const API_CHUNK_SECS: f64 = 600.0;

#[derive(Debug, Clone, Deserialize)]
pub struct Segment {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

#[derive(Debug, Clone, Default)]
pub struct AsrResult {
    pub language: Option<String>,
    pub language_probability: Option<f64>,
    pub segments: Vec<Segment>,
}

/// Hasil deteksi bahasa oleh whisper.
#[derive(Debug, Clone, Deserialize)]
pub struct Detected {
    pub language: String,
    pub language_probability: f64,
    /// Kandidat teratas (kode, probabilitas)
    #[serde(default)]
    pub candidates: Vec<(String, f64)>,
    /// Lama ucapan (detik) di sampel menurut VAD; None bila tidak diketahui
    #[serde(default)]
    pub speech_seconds: Option<f64>,
}

/// Deteksi bahasa dari file audio (sebaiknya sampel beberapa menit).
pub fn detect(cfg: &Config, audio: &Path) -> Result<Detected> {
    match cfg.asr {
        AsrBackend::Local => {
            let dir = tempfile::tempdir()?;
            let out = dir.path().join("detect.json");
            run_local(cfg, audio, &out, &["--detect"], true)?;
            serde_json::from_str(&std::fs::read_to_string(&out)?).context("hasil deteksi bahasa tidak valid")
        }
        AsrBackend::Api => {
            // API tidak punya endpoint deteksi: transkrip sampel tanpa bahasa, pakai field `language`.
            let r = api(cfg, audio, None, None)?;
            let name = r.language.unwrap_or_default();
            let code = crate::lang::code_from_name(&name).unwrap_or(name);
            Ok(Detected { language: code, language_probability: f64::NAN, candidates: vec![], speech_seconds: None })
        }
    }
}

pub fn transcribe(
    cfg: &Config,
    audio: &Path,
    language: Option<&str>,
    prompt: Option<&str>,
    quiet: bool,
) -> Result<AsrResult> {
    match cfg.asr {
        AsrBackend::Local => local(cfg, audio, language, prompt, quiet),
        AsrBackend::Api => api(cfg, audio, language, prompt),
    }
}

/// Jalankan skrip faster-whisper dengan argumen tambahan; hasil ditulis ke `out`.
fn run_local(cfg: &Config, audio: &Path, out: &Path, extra: &[&str], quiet: bool) -> Result<()> {
    let py = cfg.whisper_python();
    if !py.exists() {
        bail!(
            "Python faster-whisper tidak ditemukan di {}.\nSet VIDSUB_WHISPER_PYTHON, atau pakai --asr api.",
            py.display()
        );
    }
    let dir = tempfile::tempdir()?;
    let script = dir.path().join("vidsub_whisper.py");
    std::fs::write(&script, LOCAL_SCRIPT)?;
    let mut c = util::command(&py);
    // -X utf8: stdout/stderr & open() default ke UTF-8 (di Windows default-nya cp1252 dan merusak teks CJK).
    // Dipakai sebagai opsi, bukan PYTHONUTF8, karena -I mengabaikan variabel lingkungan PYTHON*.
    c.args(["-I", "-X", "utf8"])
        .arg(&script)
        .arg("--input")
        .arg(audio)
        .arg("--out")
        .arg(out)
        .args(["--model", &cfg.whisper_model])
        .args(["--device", &cfg.whisper_device])
        .args(["--compute-type", &cfg.whisper_compute])
        .args(["--threads", &cfg.whisper_threads().to_string()])
        .args(extra);
    if quiet {
        c.arg("--quiet");
    }
    util::run_streaming(&mut c, "faster-whisper", |line| {
        let t = line.trim();
        if let Some((pos, text)) = parse_progress(t) {
            report::progress(Some(pos), text);
        } else if let Some(msg) = t.strip_prefix("[whisper] ") {
            report::log(msg.to_string());
        } else if !t.is_empty() && !quiet {
            report::log(t.to_string());
        }
    })
}

fn local(
    cfg: &Config,
    audio: &Path,
    language: Option<&str>,
    prompt: Option<&str>,
    quiet: bool,
) -> Result<AsrResult> {
    let dir = tempfile::tempdir()?;
    let out = dir.path().join("result.json");
    let mut extra: Vec<&str> = Vec::new();
    if cfg.whisper_vad {
        extra.push("--vad");
    }
    if let Some(l) = language {
        extra.extend(["--language", l]);
    }
    if let Some(p) = prompt.filter(|p| !p.trim().is_empty()) {
        extra.extend(["--prompt", p]);
    }
    run_local(cfg, audio, &out, &extra, quiet)?;

    #[derive(Deserialize)]
    struct Out {
        language: Option<String>,
        language_probability: Option<f64>,
        segments: Vec<Segment>,
    }
    let o: Out = serde_json::from_str(&std::fs::read_to_string(&out)?)
        .context("hasil faster-whisper tidak valid")?;
    if !quiet {
        report::progress(Some(1.0), "selesai");
    }
    Ok(AsrResult {
        language: o.language,
        language_probability: o.language_probability,
        segments: o.segments,
    })
}

fn api(cfg: &Config, audio: &Path, language: Option<&str>, prompt: Option<&str>) -> Result<AsrResult> {
    let key = cfg
        .asr_api_key
        .clone()
        .filter(|k| !k.is_empty())
        .ok_or_else(|| anyhow!("VIDSUB_ASR_API_KEY belum di-set untuk --asr api"))?;
    let url = format!("{}/audio/transcriptions", cfg.asr_api_url.trim_end_matches('/'));
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(900))
        .build()?;

    let total = audio::duration(audio)?;
    let dir = tempfile::tempdir()?;
    let n_chunks = (total / API_CHUNK_SECS).ceil().max(1.0) as usize;
    let mut result = AsrResult::default();

    for i in 0..n_chunks {
        let offset = i as f64 * API_CHUNK_SECS;
        let len = API_CHUNK_SECS.min(total - offset);
        if len <= 0.05 {
            break;
        }
        report::check()?;
        report::progress(Some(i as f64 / n_chunks as f64), format!("potongan {}/{} diunggah", i + 1, n_chunks));
        let chunk = dir.path().join(format!("chunk{i}.mp3"));
        audio::cut(audio, &chunk, offset, len, true)?;

        let mut tries = 0;
        let v: Value = loop {
            tries += 1;
            let mut form = reqwest::blocking::multipart::Form::new()
                .text("model", cfg.asr_api_model.clone())
                .text("response_format", "verbose_json")
                .text("timestamp_granularities[]", "segment")
                .text("temperature", "0")
                .file("file", &chunk)?;
            if let Some(l) = language {
                form = form.text("language", l.to_string());
            }
            if let Some(p) = prompt.filter(|p| !p.trim().is_empty()) {
                form = form.text("prompt", p.to_string());
            }
            let resp = client.post(&url).bearer_auth(&key).multipart(form).send();
            match resp {
                Ok(r) if r.status().is_success() => break r.json()?,
                Ok(r) if tries < 4 && (r.status().as_u16() == 429 || r.status().is_server_error()) => {
                    report::warn(format!("HTTP {}, tunggu lalu coba lagi", r.status()));
                }
                Ok(r) => {
                    let s = r.status();
                    bail!("ASR API HTTP {s}: {}", r.text().unwrap_or_default());
                }
                Err(e) if tries < 4 => report::warn(format!("{e}, coba lagi")),
                Err(e) => return Err(e.into()),
            }
            std::thread::sleep(Duration::from_secs(5 * tries));
        };

        if result.language.is_none() {
            result.language = v["language"].as_str().map(str::to_string);
        }
        for s in v["segments"].as_array().into_iter().flatten() {
            let text = s["text"].as_str().unwrap_or("").trim().to_string();
            if text.is_empty() {
                continue;
            }
            result.segments.push(Segment {
                start: s["start"].as_f64().unwrap_or(0.0) + offset,
                end: s["end"].as_f64().unwrap_or(0.0) + offset,
                text,
            });
        }
    }
    report::progress(Some(1.0), "selesai");
    Ok(result)
}

fn parse_ts(s: &str) -> Option<f64> {
    let p: Vec<f64> = s.trim().split(':').map(|x| x.parse().ok()).collect::<Option<_>>()?;
    match p.as_slice() {
        [h, m, s] => Some(h * 3600.0 + m * 60.0 + s),
        [m, s] => Some(m * 60.0 + s),
        _ => None,
    }
}

/// "[00:00:02.32 / 00:01:15.02] teks" -> (fraksi, teks)
fn parse_progress(line: &str) -> Option<(f64, String)> {
    let rest = line.strip_prefix('[')?;
    let (times, text) = rest.split_once("] ")?;
    let (a, b) = times.split_once(" / ")?;
    let (a, b) = (parse_ts(a)?, parse_ts(b)?);
    (b > 0.0).then(|| (a / b, text.to_string()))
}

#[cfg(test)]
mod tests {
    #[test]
    fn progress_line() {
        let (f, t) = super::parse_progress("[00:00:30.00 / 00:01:00.00] halo").unwrap();
        assert!((f - 0.5).abs() < 1e-9);
        assert_eq!(t, "halo");
        assert!(super::parse_progress("[whisper] memuat").is_none());
    }
}
