use crate::config::Config;
use crate::report;
use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Map, Value, json};
use std::time::Duration;

pub struct Llm {
    client: reqwest::blocking::Client,
    url: String,
    key: Option<String>,
    pub model: String,
    temperature: f32,
    json_mode: bool,
}

impl Llm {
    pub fn new(cfg: &Config) -> Result<Self> {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(900))
            .connect_timeout(Duration::from_secs(15))
            .build()?;
        Ok(Self {
            client,
            url: format!("{}/chat/completions", cfg.llm_url.trim_end_matches('/')),
            key: cfg.llm_key.clone().filter(|k| !k.is_empty()),
            model: cfg.llm_model.clone(),
            temperature: cfg.llm_temperature,
            json_mode: cfg.llm_json_mode,
        })
    }

    pub fn chat(&self, system: &str, user: &str, want_json: bool) -> Result<String> {
        let mut body = json!({
            "model": self.model,
            "temperature": self.temperature,
            "stream": false,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": user},
            ],
        });
        if want_json && self.json_mode {
            body["response_format"] = json!({"type": "json_object"});
        }

        let mut last_err = anyhow!("tidak ada percobaan");
        for attempt in 0..4 {
            report::check()?;
            if attempt > 0 {
                std::thread::sleep(Duration::from_secs(2u64.pow(attempt)));
            }
            let mut req = self.client.post(&self.url).json(&body);
            if let Some(k) = &self.key {
                req = req.bearer_auth(k);
            }
            let resp = match req.send() {
                Ok(r) => r,
                Err(e) => {
                    let hint = if e.is_connect() {
                        " — server LLM tidak bisa dihubungi (Ollama sudah jalan? cek --llm-url)"
                    } else {
                        ""
                    };
                    last_err = anyhow!("request LLM gagal: {e}{hint}");
                    continue;
                }
            };
            let status = resp.status();
            let text = resp.text().unwrap_or_default();
            if status.as_u16() == 429 || status.is_server_error() {
                last_err = anyhow!("LLM HTTP {status}: {}", truncate(&text, 300));
                continue;
            }
            if !status.is_success() {
                bail!("LLM HTTP {status}: {}", truncate(&text, 500));
            }
            let v: Value = serde_json::from_str(&text)
                .with_context(|| format!("respon LLM bukan JSON: {}", truncate(&text, 300)))?;
            let content = v["choices"][0]["message"]["content"]
                .as_str()
                .ok_or_else(|| anyhow!("respon LLM tanpa content: {}", truncate(&text, 300)))?;
            return Ok(strip_think(content));
        }
        Err(last_err)
    }

    /// Muat model di server lebih awal (di background) supaya request pertama tidak menunggu
    /// model dimuat ke RAM. Error diabaikan.
    pub fn warmup(&self) {
        let client = self.client.clone();
        let url = self.url.clone();
        let key = self.key.clone();
        let body = json!({
            "model": self.model,
            "max_tokens": 1,
            "stream": false,
            "messages": [{"role": "user", "content": "hi"}],
        });
        std::thread::spawn(move || {
            let mut req = client.post(&url).json(&body);
            if let Some(k) = &key {
                req = req.bearer_auth(k);
            }
            let _ = req.send();
        });
    }

    pub fn chat_json(&self, system: &str, user: &str) -> Result<Value> {
        let s = self.chat(system, user, true)?;
        extract_json(&s)
    }
}

/// Daftar model dari endpoint OpenAI-compatible `/models` (Ollama juga mendukung).
pub fn list_models(cfg: &Config) -> Result<Vec<String>> {
    let client = reqwest::blocking::Client::builder().timeout(Duration::from_secs(8)).build()?;
    let mut req = client.get(format!("{}/models", cfg.llm_url.trim_end_matches('/')));
    if let Some(k) = cfg.llm_key.as_deref().filter(|k| !k.is_empty()) {
        req = req.bearer_auth(k);
    }
    let resp = req.send().map_err(|e| anyhow!("server LLM tidak bisa dihubungi: {e}"))?;
    if !resp.status().is_success() {
        bail!("server LLM HTTP {}", resp.status());
    }
    let v: Value = resp.json()?;
    let mut models: Vec<String> = v["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| m["id"].as_str().map(str::to_string))
        .collect();
    models.sort();
    Ok(models)
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(n).collect::<String>())
    }
}

/// Buang blok <think>...</think> dari model reasoning.
fn strip_think(s: &str) -> String {
    let mut out = s.to_string();
    while let Some(a) = out.find("<think>") {
        match out[a..].find("</think>") {
            Some(b) => out.replace_range(a..a + b + "</think>".len(), ""),
            None => {
                out.truncate(a);
                break;
            }
        }
    }
    out.trim().to_string()
}

pub fn extract_json(s: &str) -> Result<Value> {
    let s = s.trim();
    if let Ok(v) = serde_json::from_str::<Value>(s) {
        return Ok(v);
    }
    if let (Some(a), Some(b)) = (s.find('{'), s.rfind('}'))
        && a < b
        && let Ok(v) = serde_json::from_str::<Value>(&s[a..=b])
    {
        return Ok(v);
    }
    bail!("output LLM bukan JSON valid: {}", truncate(s, 200))
}

/// Ambil teks dari value: "teks" atau {"text": "teks"}.
fn value_text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Object(o) if o.len() == 1 => o.values().next().and_then(value_text),
        Value::Object(o) => ["text", "translation", "line"]
            .iter()
            .find_map(|k| o.get(*k).and_then(value_text)),
        _ => None,
    }
}

/// Kalau model membungkus hasil, mis. {"lines": {...}}, buka bungkusnya.
fn unwrap_obj(v: Value) -> Map<String, Value> {
    match v {
        Value::Object(o) => {
            if !o.contains_key("1")
                && o.len() == 1
                && let Some(Value::Object(inner)) = o.values().next()
            {
                return inner.clone();
            }
            o
        }
        Value::Array(arr) => arr
            .into_iter()
            .enumerate()
            .map(|(i, v)| ((i + 1).to_string(), v))
            .collect(),
        _ => Map::new(),
    }
}

/// Cara memvalidasi keluaran LLM per baris.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    /// LLM menyalin teks sumber per baris (`src`) supaya pergeseran baris terdeteksi.
    /// Dipakai untuk translate (bahasa berbeda, jadi kemiripan teks tidak bisa dipakai).
    Echo,
    /// Hanya teks hasil. Lebih cepat (token keluaran ~setengah). Pemanggil wajib
    /// memvalidasi sendiri, mis. guard kemiripan untuk perapian dalam bahasa yang sama.
    Plain,
}

fn build_user(context: &[String], obj: &Map<String, Value>, mode: Mode) -> String {
    let mut u = String::new();
    if !context.is_empty() {
        u.push_str("Previous lines for context only (do NOT include them in the output):\n");
        for c in context {
            u.push_str("- ");
            u.push_str(&c.replace('\n', " "));
            u.push('\n');
        }
        u.push('\n');
    }
    u.push_str("Lines to process:\n");
    u.push_str(&serde_json::to_string_pretty(&Value::Object(obj.clone())).unwrap());
    u.push_str(match mode {
        Mode::Echo => {
            "\n\nReturn ONLY a JSON object with exactly the same keys. Each value must be an object \
{\"src\": <the original line for that key, copied exactly>, \"out\": <the processed line>}. \
Process every key independently: \"out\" for a key may only contain the content of that key's own line."
        }
        Mode::Plain => {
            "\n\nReturn ONLY a JSON object with exactly the same keys, each value being the processed line. \
Process every key independently: the value for a key may only contain the content of that key's own line."
        }
    });
    u
}

/// Hasil satu baris dari LLM: teks keluaran + salinan sumber (untuk cek pergeseran).
struct Item {
    out: String,
    src: Option<String>,
}

fn item_of(v: &Value) -> Option<Item> {
    match v {
        Value::String(s) => Some(Item { out: s.clone(), src: None }),
        Value::Object(o) => {
            let out = ["out", "text", "translation", "line", "output"]
                .iter()
                .find_map(|k| o.get(*k).and_then(Value::as_str))
                .map(str::to_string)
                .or_else(|| (o.len() == 1).then(|| o.values().next().and_then(value_text)).flatten())?;
            let src = ["src", "source", "original"]
                .iter()
                .find_map(|k| o.get(*k).and_then(Value::as_str))
                .map(str::to_string);
            Some(Item { out, src })
        }
        _ => None,
    }
}

/// Salinan sumber cocok dengan baris aslinya? (mendeteksi baris yang bergeser)
fn echo_ok(original: &str, echo: Option<&str>) -> bool {
    let Some(echo) = echo else { return true };
    let norm = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
    if norm(original) == norm(echo) {
        return true;
    }
    crate::util::dice(original, echo).is_none_or(|d| d >= 0.7)
}

/// Proses banyak baris lewat LLM per-batch. Urutan & jumlah baris selalu sama dengan input;
/// baris yang gagal diproses dikembalikan apa adanya (dengan peringatan).
pub fn map_lines(
    llm: &Llm,
    system: &str,
    texts: &[String],
    batch: usize,
    label: &str,
    mode: Mode,
) -> Result<Vec<String>> {
    let batch = batch.max(1);
    let total_batches = texts.len().div_ceil(batch);
    let mut out = Vec::with_capacity(texts.len());
    for (bi, start) in (0..texts.len()).step_by(batch).enumerate() {
        let end = (start + batch).min(texts.len());
        report::check()?;
        report::progress(
            Some(start as f64 / texts.len().max(1) as f64),
            format!("{label}: batch {}/{} (baris {}-{} dari {})", bi + 1, total_batches, start + 1, end, texts.len()),
        );
        let ctx = &texts[start.saturating_sub(3)..start];
        out.extend(process_batch(llm, system, &texts[start..end], ctx, mode)?);
    }
    report::progress(Some(1.0), format!("{label}: {} baris selesai", texts.len()));
    Ok(out)
}

fn char_len(s: &str) -> f64 {
    s.chars().filter(|c| !c.is_whitespace()).count().max(1) as f64
}

fn process_batch(llm: &Llm, system: &str, lines: &[String], ctx: &[String], mode: Mode) -> Result<Vec<String>> {
    let mut result: Vec<Option<String>> = lines
        .iter()
        .map(|l| l.trim().is_empty().then(String::new))
        .collect();

    // idx: indeks baris di batch ini; ctx_for: konteks yang dikirim
    let request = |idx: &[usize], ctx_for: &[String]| -> Result<Map<String, Value>> {
        let obj: Map<String, Value> = idx
            .iter()
            .enumerate()
            .map(|(k, &i)| ((k + 1).to_string(), Value::String(lines[i].clone())))
            .collect();
        Ok(unwrap_obj(llm.chat_json(system, &build_user(ctx_for, &obj, mode))?))
    };

    let mut last_err = None;
    let mut any_ok = false;
    let attempts = if mode == Mode::Echo { 3 } else { 2 };
    for attempt in 1..=attempts {
        let pending: Vec<usize> = (0..lines.len()).filter(|&i| result[i].is_none()).collect();
        if pending.is_empty() {
            break;
        }
        match request(&pending, ctx) {
            Ok(map) => {
                any_ok = true;
                let mut shifted = 0;
                for (k, &i) in pending.iter().enumerate() {
                    let Some(item) = map.get(&(k + 1).to_string()).and_then(item_of) else {
                        continue;
                    };
                    if !echo_ok(&lines[i], item.src.as_deref()) {
                        shifted += 1;
                        continue;
                    }
                    let t = item.out.trim().to_string();
                    if !t.is_empty() {
                        result[i] = Some(t);
                    }
                }
                let missing = result.iter().filter(|r| r.is_none()).count();
                if missing > 0 {
                    report::warn(format!(
                        "{missing} baris belum valid ({shifted} bergeser), coba lagi ({attempt}/{attempts})"
                    ));
                }
            }
            Err(e) => {
                if report::cancelled() {
                    return Err(e);
                }
                report::warn(format!("percobaan {attempt}/{attempts} gagal: {e}"));
                last_err = Some(e);
            }
        }
    }
    if !any_ok && let Some(e) = last_err {
        return Err(e.context("LLM gagal berturut-turut, proses dihentikan"));
    }

    if mode == Mode::Plain {
        // Validasi diserahkan ke pemanggil; baris yang tidak dikembalikan tetap asli.
        return Ok(result.into_iter().zip(lines).map(|(r, l)| r.unwrap_or_else(|| l.clone())).collect());
    }

    // Baris yang panjangnya janggal dibanding baris lain (tanda penggabungan) diulang satu per satu.
    let mut ratios: Vec<f64> = (0..lines.len())
        .filter_map(|i| result[i].as_ref().filter(|_| char_len(&lines[i]) >= 12.0).map(|r| char_len(r) / char_len(&lines[i])))
        .collect();
    ratios.sort_by(|a, b| a.total_cmp(b));
    let median = ratios.get(ratios.len() / 2).copied();
    let mut redo: Vec<usize> = (0..lines.len()).filter(|&i| result[i].is_none()).collect();
    if let Some(m) = median.filter(|_| ratios.len() >= 4) {
        for i in 0..lines.len() {
            if let Some(r) = &result[i]
                && char_len(&lines[i]) >= 12.0
            {
                let ratio = char_len(r) / char_len(&lines[i]);
                if ratio > m * 1.9 || ratio < m / 2.2 {
                    redo.push(i);
                }
            }
        }
    }

    // Satu per satu, dengan konteks baris sebelumnya.
    for i in redo {
        report::check()?;
        let before: Vec<String> = if i >= 3 { lines[i - 3..i].to_vec() } else {
            ctx.iter().rev().take(3 - i).rev().cloned().chain(lines[..i].iter().cloned()).collect()
        };
        match request(&[i], &before) {
            Ok(map) => {
                if let Some(item) = map.get("1").and_then(item_of).filter(|it| !it.out.trim().is_empty()) {
                    result[i] = Some(item.out.trim().to_string());
                }
            }
            Err(e) => report::warn(format!("baris gagal: {e}")),
        }
        if result[i].as_deref().is_none_or(str::is_empty) {
            report::warn(format!("baris dibiarkan asli: {}", lines[i].replace('\n', " ")));
            result[i] = Some(lines[i].clone());
        }
    }

    Ok(result.into_iter().map(|r| r.unwrap_or_default()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_extraction() {
        let v = extract_json("<x>```json\n{\"1\": \"a\"}\n```").unwrap();
        assert_eq!(v["1"], "a");
        assert_eq!(strip_think("<think>hmm</think>{\"a\":1}"), "{\"a\":1}");
        let m = unwrap_obj(json!({"lines": {"1": "x"}}));
        assert_eq!(m["1"], "x");
        assert_eq!(value_text(&json!({"text": "y", "id": 1})).unwrap(), "y");
        let it = item_of(&json!({"src": "a b", "out": "c"})).unwrap();
        assert_eq!((it.out.as_str(), it.src.as_deref()), ("c", Some("a b")));
    }

    #[test]
    fn echo_detects_shift() {
        assert!(echo_ok("and I will assure you", Some("and I will assure you")));
        assert!(!echo_ok(
            "and I will assure you that my first lecture will be very brief.",
            "I am delighted to be here, and I'm particularly delighted to be here on this".into()
        ));
        assert!(echo_ok("Hi", None));
    }
}
