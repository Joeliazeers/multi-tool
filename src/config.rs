use clap::{ArgAction, Args, Parser, ValueEnum};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AsrBackend {
    /// faster-whisper lokal (venv Python)
    Local,
    /// API OpenAI-compatible /audio/transcriptions (Groq, OpenAI, speaches, dll)
    Api,
}

/// Semua konfigurasi bisa lewat flag, environment variable, atau file .env
/// (./.env atau ~/.config/vidsub/config.env).
#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[command(next_help_heading = "Konfigurasi (bisa juga via env / .env)")]
pub struct Config {
    /// Base URL API LLM OpenAI-compatible (Ollama, OpenRouter, LM Studio, dll)
    #[arg(long, env = "VIDSUB_LLM_URL", default_value = "http://localhost:11434/v1", global = true)]
    pub llm_url: String,

    /// API key LLM (kosongkan untuk Ollama lokal)
    #[arg(long, env = "VIDSUB_LLM_KEY", hide_env_values = true, global = true)]
    pub llm_key: Option<String>,

    /// Nama model LLM
    #[arg(long, env = "VIDSUB_LLM_MODEL", default_value = "qwen2.5:7b-instruct", global = true)]
    pub llm_model: String,

    #[arg(long, env = "VIDSUB_LLM_TEMPERATURE", default_value_t = 0.3, global = true)]
    pub llm_temperature: f32,

    /// Kirim response_format=json_object ke LLM (matikan jika provider menolak)
    #[arg(long, env = "VIDSUB_LLM_JSON_MODE", default_value_t = true, action = ArgAction::Set, global = true)]
    pub llm_json_mode: bool,

    /// Jumlah baris subtitle per request LLM
    #[arg(long, env = "VIDSUB_BATCH", default_value_t = 25, global = true)]
    pub batch_size: usize,

    /// Backend speech-to-text
    #[arg(long, env = "VIDSUB_ASR", value_enum, default_value_t = AsrBackend::Local, global = true)]
    pub asr: AsrBackend,

    /// Python dari venv yang punya faster-whisper (default: ~/whisper-env/bin/python, Windows: ~\\whisper-env\\Scripts\\python.exe)
    #[arg(long, env = "VIDSUB_WHISPER_PYTHON", global = true)]
    pub whisper_python: Option<PathBuf>,

    #[arg(long, env = "VIDSUB_WHISPER_MODEL", default_value = "large-v3", global = true)]
    pub whisper_model: String,

    #[arg(long, env = "VIDSUB_WHISPER_DEVICE", default_value = "cpu", global = true)]
    pub whisper_device: String,

    #[arg(long, env = "VIDSUB_WHISPER_COMPUTE", default_value = "int8", global = true)]
    pub whisper_compute: String,

    /// Lewati bagian hening dengan VAD: lebih cepat, tapi bisa melewatkan dialog pelan/berbisik
    #[arg(long, env = "VIDSUB_WHISPER_VAD", default_value_t = false, action = ArgAction::Set, global = true)]
    #[serde(default)]
    pub whisper_vad: bool,

    /// Thread CPU untuk whisper (0 = semua core)
    #[arg(long, env = "VIDSUB_WHISPER_THREADS", default_value_t = 0, global = true)]
    pub whisper_threads: usize,

    #[arg(long, env = "VIDSUB_ASR_API_URL", default_value = "https://api.groq.com/openai/v1", global = true)]
    pub asr_api_url: String,

    #[arg(long, env = "VIDSUB_ASR_API_KEY", hide_env_values = true, global = true)]
    pub asr_api_key: Option<String>,

    #[arg(long, env = "VIDSUB_ASR_API_MODEL", default_value = "whisper-large-v3", global = true)]
    pub asr_api_model: String,

    /// Folder hasil download (default: ~/Videos)
    #[arg(long, env = "VIDSUB_OUTPUT_DIR", global = true)]
    pub output_dir: Option<PathBuf>,

    /// Jumlah koneksi paralel untuk download file langsung (1-32)
    #[arg(long, env = "VIDSUB_DL_SEGMENTS", default_value_t = 8, global = true)]
    #[serde(default = "default_segments")]
    pub download_segments: usize,

    /// Batas kecepatan download total, mis. 2M atau 500K (0 = tanpa batas)
    #[arg(long = "dl-limit", env = "VIDSUB_SPEED_LIMIT", value_parser = crate::fetch::limit::parse_size, default_value = "0", global = true)]
    #[serde(default)]
    pub speed_limit: u64,

    /// Maksimal download berjalan bersamaan di GUI
    #[arg(long, env = "VIDSUB_DL_PARALLEL", default_value_t = 3, global = true)]
    #[serde(default = "default_parallel")]
    pub download_parallel: usize,
}

fn default_segments() -> usize {
    8
}
fn default_parallel() -> usize {
    3
}

impl Config {
    pub fn whisper_python(&self) -> PathBuf {
        self.whisper_python
            .clone()
            .unwrap_or_else(|| default_whisper_python(&dirs::home_dir().unwrap_or_default(), cfg!(windows)))
    }

    pub fn output_dir(&self) -> PathBuf {
        self.output_dir.clone().unwrap_or_else(|| {
            dirs::video_dir()
                .or_else(|| dirs::home_dir().map(|h| h.join("Videos")))
                .unwrap_or_else(|| PathBuf::from("."))
        })
    }

    pub fn whisper_threads(&self) -> usize {
        if self.whisper_threads > 0 {
            self.whisper_threads
        } else {
            std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4)
        }
    }

    pub fn print(&self) {
        let mask = |k: &Option<String>| match k {
            Some(k) if !k.is_empty() => format!("(terisi, {} karakter)", k.len()),
            _ => "(kosong)".into(),
        };
        println!("LLM");
        println!("  url          : {}", self.llm_url);
        println!("  model        : {}", self.llm_model);
        println!("  api key      : {}", mask(&self.llm_key));
        println!("  temperature  : {}", self.llm_temperature);
        println!("  json mode    : {}", self.llm_json_mode);
        println!("  batch        : {} baris", self.batch_size);
        println!("ASR / transcribe");
        println!("  backend      : {:?}", self.asr);
        match self.asr {
            AsrBackend::Local => {
                println!("  python       : {}", self.whisper_python().display());
                println!(
                    "  model        : {} ({}/{}, {} thread, VAD {})",
                    self.whisper_model,
                    self.whisper_device,
                    self.whisper_compute,
                    self.whisper_threads(),
                    if self.whisper_vad { "aktif" } else { "mati" }
                );
            }
            AsrBackend::Api => {
                println!("  url          : {}", self.asr_api_url);
                println!("  model        : {}", self.asr_api_model);
                println!("  api key      : {}", mask(&self.asr_api_key));
            }
        }
        println!("Download");
        println!("  folder       : {}", self.output_dir().display());
        println!("  koneksi      : {} per file", self.download_segments);
        match self.speed_limit {
            0 => println!("  batas        : tanpa batas"),
            n => println!("  batas        : {:.1} MB/s", n as f64 / 1_048_576.0),
        }
        println!("  bersamaan    : {} download (GUI)", self.download_parallel);
    }
}

/// Python venv bawaan: `~/whisper-env/bin/python` (Linux/macOS) atau `~\whisper-env\Scripts\python.exe` (Windows).
fn default_whisper_python(home: &std::path::Path, windows: bool) -> PathBuf {
    if windows {
        home.join("whisper-env").join("Scripts").join("python.exe")
    } else {
        home.join("whisper-env/bin/python")
    }
}

/// Path file konfigurasi bersama CLI & GUI.
pub fn config_file() -> PathBuf {
    dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("vidsub/config.env")
}

/// Muat ~/.config/vidsub/config.env lalu ./.env ke environment.
pub fn load_env() {
    let _ = dotenvy::from_path(config_file());
    let _ = dotenvy::dotenv();
}

#[derive(Parser)]
struct OnlyConfig {
    #[command(flatten)]
    cfg: Config,
}

/// Konfigurasi dari environment/.env (tanpa argumen CLI), untuk GUI.
pub fn load() -> Config {
    load_env();
    OnlyConfig::try_parse_from(["multitool"]).map(|o| o.cfg).unwrap_or_else(|_| {
        // nilai env rusak: pakai default murni
        unsafe_default()
    })
}

fn unsafe_default() -> Config {
    Config {
        llm_url: "http://localhost:11434/v1".into(),
        llm_key: None,
        llm_model: "qwen2.5:7b-instruct".into(),
        llm_temperature: 0.3,
        llm_json_mode: true,
        batch_size: 25,
        asr: AsrBackend::Local,
        whisper_python: None,
        whisper_model: "large-v3".into(),
        whisper_device: "cpu".into(),
        whisper_compute: "int8".into(),
        whisper_threads: 0,
        whisper_vad: false,
        asr_api_url: "https://api.groq.com/openai/v1".into(),
        asr_api_key: None,
        asr_api_model: "whisper-large-v3".into(),
        output_dir: None,
        download_segments: 8,
        speed_limit: 0,
        download_parallel: 3,
    }
}

/// Simpan konfigurasi ke config.env. Baris lain (komentar, kunci tak dikenal) dipertahankan.
pub fn save(cfg: &Config) -> anyhow::Result<PathBuf> {
    let path = config_file();
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p)?;
    }
    let opt = |o: &Option<String>| o.clone().unwrap_or_default();
    let path_opt = |o: &Option<PathBuf>| o.as_ref().map(|p| p.display().to_string()).unwrap_or_default();
    let values: Vec<(&str, String)> = vec![
        ("VIDSUB_LLM_URL", cfg.llm_url.clone()),
        ("VIDSUB_LLM_KEY", opt(&cfg.llm_key)),
        ("VIDSUB_LLM_MODEL", cfg.llm_model.clone()),
        ("VIDSUB_LLM_TEMPERATURE", cfg.llm_temperature.to_string()),
        ("VIDSUB_LLM_JSON_MODE", cfg.llm_json_mode.to_string()),
        ("VIDSUB_BATCH", cfg.batch_size.to_string()),
        ("VIDSUB_ASR", format!("{:?}", cfg.asr).to_lowercase()),
        ("VIDSUB_WHISPER_PYTHON", path_opt(&cfg.whisper_python)),
        ("VIDSUB_WHISPER_MODEL", cfg.whisper_model.clone()),
        ("VIDSUB_WHISPER_DEVICE", cfg.whisper_device.clone()),
        ("VIDSUB_WHISPER_COMPUTE", cfg.whisper_compute.clone()),
        ("VIDSUB_WHISPER_THREADS", cfg.whisper_threads.to_string()),
        ("VIDSUB_WHISPER_VAD", cfg.whisper_vad.to_string()),
        ("VIDSUB_ASR_API_URL", cfg.asr_api_url.clone()),
        ("VIDSUB_ASR_API_KEY", opt(&cfg.asr_api_key)),
        ("VIDSUB_ASR_API_MODEL", cfg.asr_api_model.clone()),
        ("VIDSUB_OUTPUT_DIR", path_opt(&cfg.output_dir)),
        ("VIDSUB_DL_SEGMENTS", cfg.download_segments.to_string()),
        ("VIDSUB_SPEED_LIMIT", cfg.speed_limit.to_string()),
        ("VIDSUB_DL_PARALLEL", cfg.download_parallel.to_string()),
    ];
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let mut out: Vec<String> = Vec::new();
    let mut written = std::collections::HashSet::new();
    for line in existing.lines() {
        let key = line.split_once('=').map(|(k, _)| k.trim().trim_start_matches("export ").trim());
        match key.and_then(|k| values.iter().find(|(vk, _)| *vk == k)) {
            Some((k, v)) => {
                if written.insert(*k) && !v.is_empty() {
                    out.push(format!("{k}={}", quote(v)));
                }
            }
            None => out.push(line.to_string()),
        }
    }
    for (k, v) in &values {
        if !written.contains(k) && !v.is_empty() {
            out.push(format!("{k}={}", quote(v)));
        }
    }
    std::fs::write(&path, out.join("\n") + "\n")?;
    Ok(path)
}

fn quote(v: &str) -> String {
    if !v.chars().any(|c| c.is_whitespace() || "#\"'$\\".contains(c)) {
        v.to_string()
    } else if !v.contains('\'') {
        format!("'{v}'") // kutip tunggal = literal di dotenv
    } else {
        let esc = v.replace('\\', "\\\\").replace('"', "\\\"").replace('$', "\\$");
        format!("\"{esc}\"")
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    #[test]
    fn default_download_dir_is_plain_videos_folder() {
        let cfg = super::OnlyConfig::try_parse_from(["multitool"]).unwrap().cfg;
        if cfg.output_dir.is_some() {
            return; // VIDSUB_OUTPUT_DIR di environment menimpa default
        }
        let videos = dirs::video_dir().or_else(|| dirs::home_dir().map(|h| h.join("Videos"))).unwrap();
        assert_eq!(cfg.output_dir(), videos);
    }

    #[test]
    fn whisper_python_default_per_platform() {
        let home = std::path::Path::new("/home/u");
        assert_eq!(super::default_whisper_python(home, false), home.join("whisper-env/bin/python"));
        assert_eq!(
            super::default_whisper_python(home, true),
            home.join("whisper-env").join("Scripts").join("python.exe")
        );
    }

    #[test]
    fn new_download_keys_are_saved_and_old_configs_get_defaults() {
        let c = super::unsafe_default();
        assert_eq!((c.download_segments, c.speed_limit, c.download_parallel), (8, 0, 3));
        let mut json = serde_json::to_value(&c).unwrap();
        for k in ["download_segments", "speed_limit", "download_parallel"] {
            json.as_object_mut().unwrap().remove(k); // payload GUI lama
        }
        let old: super::Config = serde_json::from_value(json).unwrap();
        assert_eq!((old.download_segments, old.speed_limit, old.download_parallel), (8, 0, 3));
    }

    #[test]
    fn quoting_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("c.env");
        let vals = ["simple", "with space", "has \"quote\" and $x", "back\\slash #hash", "it's $5"];
        let body: String = vals.iter().enumerate().map(|(i, v)| format!("K{i}={}\n", super::quote(v))).collect();
        std::fs::write(&f, body).unwrap();
        let parsed: Vec<(String, String)> = dotenvy::from_path_iter(&f).unwrap().map(|r| r.unwrap()).collect();
        for (i, v) in vals.iter().enumerate() {
            assert_eq!(parsed[i].1, *v, "value {i}");
        }
    }
}
