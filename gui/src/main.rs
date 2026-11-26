#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;
use multitool::config::{self, Config};
use multitool::download::{self, DownloadArgs, VideoInfo};
use multitool::lang::{self, LANGS};
use multitool::transcribe::{self, Detection, TranscribeOptions};
use multitool::translate::{self, SrtInfo};
use multitool::{audio, llm, qr, report, util};

/// Jenis pekerjaan. Download boleh jalan bersamaan dengan apa pun; Process
/// (deteksi/transcribe/translate) memakai CPU & LLM berat, jadi hanya satu sekaligus.
#[derive(Copy, Clone, PartialEq, Eq)]
enum Kind {
    Download,
    Process,
}

/// id -> (jenis, tombol batal, "simpan sisa" untuk jeda)
type Jobs = HashMap<u64, (Kind, Arc<AtomicBool>, Arc<AtomicBool>)>;

struct AppState {
    cfg: Mutex<Config>,
    /// pekerjaan yang sedang berjalan
    jobs: Mutex<Jobs>,
}

/// Lepaskan pekerjaan dari daftar walau future dibatalkan/panik.
struct JobSlot<'a> {
    state: &'a AppState,
    id: u64,
}
impl Drop for JobSlot<'_> {
    fn drop(&mut self) {
        self.state.jobs.lock().unwrap().remove(&self.id);
    }
}

type R<T> = Result<T, String>;

fn err(e: anyhow::Error) -> String {
    format!("{e:#}")
}

/// Jalankan pekerjaan di thread terpisah dengan ID dari frontend (untuk event & batal).
async fn job<T: Send + 'static>(
    state: &State<'_, AppState>,
    id: u64,
    kind: Kind,
    f: impl FnOnce(Config) -> anyhow::Result<T> + Send + 'static,
) -> R<T> {
    let cancel = Arc::new(AtomicBool::new(false));
    let keep = Arc::new(AtomicBool::new(false));
    {
        let mut jobs = state.jobs.lock().unwrap();
        if kind == Kind::Process && jobs.values().any(|(k, _, _)| *k == Kind::Process) {
            return Err("Masih ada transcribe/translate yang berjalan.".into());
        }
        if jobs.contains_key(&id) {
            return Err(format!("ID pekerjaan {id} sudah dipakai"));
        }
        jobs.insert(id, (kind, cancel.clone(), keep.clone()));
    }
    let _slot = JobSlot { state, id };
    let cfg = state.cfg.lock().unwrap().clone();
    let ctx = report::Job { id, cancel, keep };
    let res = tauri::async_runtime::spawn_blocking(move || report::with_job(ctx, || f(cfg))).await;
    match res {
        Ok(r) => r.map_err(err),
        Err(e) => Err(format!("proses gagal: {e}")),
    }
}

/// Pekerjaan ringan (tidak mengunci tombol utama).
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> anyhow::Result<T> + Send + 'static) -> R<T> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| e.to_string())?
        .map_err(err)
}

fn opt_path(s: Option<String>) -> Option<PathBuf> {
    s.filter(|s| !s.trim().is_empty()).map(|s| util::clean_path(&s))
}

// ------------------------------------------------------------------ pengaturan

#[derive(Serialize)]
struct Settings {
    cfg: Config,
    output_dir: String,
    whisper_python: String,
    config_path: String,
}

#[tauri::command]
fn get_settings(state: State<'_, AppState>) -> Settings {
    let cfg = state.cfg.lock().unwrap().clone();
    Settings {
        output_dir: cfg.output_dir().display().to_string(),
        whisper_python: cfg.whisper_python().display().to_string(),
        config_path: config::config_file().display().to_string(),
        cfg,
    }
}

#[tauri::command]
fn save_settings(state: State<'_, AppState>, cfg: Config) -> R<String> {
    if cfg.batch_size == 0 {
        return Err("Batch minimal 1".into());
    }
    if !(1..=32).contains(&cfg.download_segments) {
        return Err("Jumlah koneksi download harus 1-32".into());
    }
    if !(1..=10).contains(&cfg.download_parallel) {
        return Err("Download bersamaan harus 1-10".into());
    }
    let path = config::save(&cfg).map_err(err)?;
    *state.cfg.lock().unwrap() = cfg;
    Ok(path.display().to_string())
}

/// Daftar model di server LLM. `url`/`key` boleh diisi untuk mencoba nilai yang belum disimpan.
#[tauri::command]
async fn list_models(state: State<'_, AppState>, url: Option<String>, key: Option<String>) -> R<Vec<String>> {
    let mut cfg = state.cfg.lock().unwrap().clone();
    if let Some(u) = url.filter(|u| !u.trim().is_empty()) {
        cfg.llm_url = u.trim().to_string();
        cfg.llm_key = key.filter(|k| !k.trim().is_empty());
    }
    blocking(move || llm::list_models(&cfg)).await
}

#[derive(Serialize)]
struct LangItem {
    code: &'static str,
    name: &'static str,
    local: &'static str,
}

#[tauri::command]
fn languages() -> Vec<LangItem> {
    LANGS.iter().map(|l| LangItem { code: l.0, name: l.1, local: l.2 }).collect()
}

#[derive(Serialize)]
struct Dep {
    name: String,
    ok: bool,
    detail: String,
}

fn version_of(bin: &str, arg: &str) -> Dep {
    match util::command(bin).arg(arg).output() {
        Ok(o) if o.status.success() => Dep {
            name: bin.into(),
            ok: true,
            // "ffmpeg version 6.1.1-3ubuntu5 Copyright ..." -> "6.1.1-3ubuntu5"
            detail: String::from_utf8_lossy(&o.stdout)
                .lines()
                .next()
                .unwrap_or("")
                .trim_start_matches("ffmpeg version ")
                .split_whitespace()
                .next()
                .unwrap_or("")
                .to_string(),
        },
        _ => Dep { name: bin.into(), ok: false, detail: "tidak ditemukan di PATH".into() },
    }
}

#[tauri::command]
async fn check_deps(state: State<'_, AppState>) -> R<Vec<Dep>> {
    let cfg = state.cfg.lock().unwrap().clone();
    blocking(move || {
        let mut deps = vec![version_of("yt-dlp", "--version"), version_of("ffmpeg", "-version")];
        if cfg.asr == config::AsrBackend::Local {
            let py = cfg.whisper_python();
            let ok = util::command(&py)
                .args(["-I", "-X", "utf8", "-c", "import faster_whisper,sys;print(faster_whisper.__version__)"])
                .output()
                .ok()
                .filter(|o| o.status.success());
            deps.push(Dep {
                name: "faster-whisper".into(),
                ok: ok.is_some(),
                detail: match ok {
                    Some(o) => format!("v{} · {}", String::from_utf8_lossy(&o.stdout).trim(), cfg.whisper_model),
                    None => format!("tidak bisa dijalankan: {}", py.display()),
                },
            });
        } else {
            let has_key = cfg.asr_api_key.as_deref().is_some_and(|k| !k.is_empty());
            deps.push(Dep {
                name: "Whisper API".into(),
                ok: has_key,
                detail: if has_key { cfg.asr_api_url.clone() } else { "API key belum diisi".into() },
            });
        }
        let llm = match llm::list_models(&cfg) {
            Ok(models) => {
                let has = models.iter().any(|m| m == &cfg.llm_model);
                Dep {
                    name: "Server LLM".into(),
                    ok: has,
                    detail: if has {
                        format!("{} · {}", cfg.llm_url, cfg.llm_model)
                    } else {
                        format!("model '{}' tidak ada di server", cfg.llm_model)
                    },
                }
            }
            Err(e) => Dep { name: "Server LLM".into(), ok: false, detail: e.to_string() },
        };
        deps.push(llm);
        Ok(deps)
    })
    .await
}

// ------------------------------------------------------------------ file & dialog

#[derive(Serialize)]
struct MediaInfo {
    path: String,
    name: String,
    dir: String,
    stem: String,
    size: u64,
    duration: Option<f64>,
}

#[tauri::command]
async fn media_info(path: String) -> R<MediaInfo> {
    blocking(move || {
        let p = util::clean_path(&path);
        let meta = std::fs::metadata(&p).map_err(|_| anyhow::anyhow!("File tidak ditemukan: {}", p.display()))?;
        if !meta.is_file() {
            anyhow::bail!("Bukan file: {}", p.display());
        }
        Ok(MediaInfo {
            name: p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            dir: p.parent().map(|d| d.display().to_string()).unwrap_or_default(),
            stem: util::base_stem(&p),
            size: meta.len(),
            duration: audio::duration(&p).ok(),
            path: p.display().to_string(),
        })
    })
    .await
}

#[tauri::command]
async fn srt_info(path: String) -> R<SrtInfo> {
    blocking(move || translate::info(&util::clean_path(&path))).await
}

fn start_dir(start: Option<String>) -> PathBuf {
    opt_path(start)
        .map(|p| if p.is_file() { p.parent().map(Path::to_path_buf).unwrap_or(p) } else { p })
        .filter(|p| p.is_dir())
        .or_else(dirs_home)
        .unwrap_or_else(|| PathBuf::from("/"))
}

fn dirs_home() -> Option<PathBuf> {
    dirs::home_dir()
}

#[tauri::command]
async fn pick_folder(app: tauri::AppHandle, start: Option<String>) -> Option<String> {
    app.dialog()
        .file()
        .set_title("Pilih folder")
        .set_directory(start_dir(start))
        .blocking_pick_folder()
        .and_then(|f| f.into_path().ok())
        .map(|p| p.display().to_string())
}

#[tauri::command]
async fn pick_file(app: tauri::AppHandle, kind: String, start: Option<String>) -> Option<String> {
    let mut d = app.dialog().file().set_directory(start_dir(start));
    d = if kind == "srt" {
        d.set_title("Pilih file SRT").add_filter("Subtitle SRT", &["srt", "SRT"])
    } else {
        d.set_title("Pilih video atau audio").add_filter(
            "Video/Audio",
            &["mp4", "mkv", "webm", "mov", "avi", "flv", "ts", "m4v", "mp3", "m4a", "wav", "ogg", "opus", "flac", "aac"],
        )
    };
    d.add_filter("Semua file", &["*"])
        .blocking_pick_file()
        .and_then(|f| f.into_path().ok())
        .map(|p| p.display().to_string())
}

#[tauri::command]
async fn save_srt(app: tauri::AppHandle, suggested: String) -> Option<String> {
    let p = util::clean_path(&suggested);
    let mut d = app.dialog().file().set_title("Simpan SRT sebagai").add_filter("Subtitle SRT", &["srt"]);
    if let Some(dir) = p.parent().filter(|d| d.is_dir()) {
        d = d.set_directory(dir);
    }
    if let Some(name) = p.file_name() {
        d = d.set_file_name(name.to_string_lossy());
    }
    d.blocking_save_file()
        .and_then(|f| f.into_path().ok())
        .map(|mut p| {
            if p.extension().is_none() {
                p.set_extension("srt");
            }
            p.display().to_string()
        })
}

// ------------------------------------------------------------------ QR code

/// Pratinjau QR sebagai data URL PNG. Dirender di thread terpisah dan maksimal 512 px
/// (gambar di UI diperkecil juga), supaya ukuran besar tidak membekukan jendela.
#[tauri::command]
async fn qr_preview(text: String, mut opts: qr::QrOptions) -> R<String> {
    opts.size = opts.size.min(512);
    blocking(move || qr::png_data_url(&text, &opts)).await
}

/// Simpan QR ke `path`; format dari ekstensi (tanpa ekstensi = PNG).
#[tauri::command]
async fn qr_save(text: String, opts: qr::QrOptions, path: String) -> R<String> {
    blocking(move || Ok(qr::save(&text, &opts, &util::clean_path(&path))?.display().to_string())).await
}

#[tauri::command]
async fn pick_save_qr(app: tauri::AppHandle, suggested: Option<String>) -> Option<String> {
    app.dialog()
        .file()
        .set_title("Simpan QR code sebagai")
        .set_directory(start_dir(suggested))
        .set_file_name("qr.png")
        .add_filter("PNG", &["png"])
        .add_filter("JPEG", &["jpg", "jpeg"])
        .add_filter("SVG", &["svg"])
        .add_filter("WebP", &["webp"])
        .add_filter("BMP", &["bmp"])
        .blocking_save_file()
        .and_then(|f| f.into_path().ok())
        .map(|p| p.display().to_string())
}

/// Buka file/folder dengan aplikasi bawaan. `reveal` = tampilkan file di folder yang berisi file itu.
#[tauri::command]
fn open_path(path: String, reveal: Option<bool>) -> R<()> {
    let p = util::clean_path(&path);
    let reveal = reveal.unwrap_or(false);
    let target = if reveal || !p.exists() { p.parent().map(Path::to_path_buf).unwrap_or_else(|| p.clone()) } else { p.clone() };
    #[cfg(windows)]
    let mut cmd = if reveal && p.is_file() {
        let mut c = Command::new("explorer");
        c.arg(format!("/select,{}", p.display())); // satu argumen: "/select,C:\\dir\\file"
        c
    } else {
        let mut c = Command::new("explorer");
        c.arg(&target);
        c
    };
    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut c = Command::new("open");
        c.arg(&target);
        c
    };
    #[cfg(not(any(windows, target_os = "macos")))]
    let mut cmd = {
        let mut c = Command::new("xdg-open");
        c.arg(&target);
        c
    };
    cmd.spawn().map_err(|e| e.to_string())?;
    Ok(())
}

// ------------------------------------------------------------------ pekerjaan

#[tauri::command]
async fn probe(args: DownloadArgs) -> R<VideoInfo> {
    blocking(move || download::probe(&args)).await
}

#[tauri::command]
async fn download(state: State<'_, AppState>, job_id: u64, args: DownloadArgs) -> R<Vec<String>> {
    job(&state, job_id, Kind::Download, move |cfg| {
        let files = download::run(&cfg, &args)?;
        if files.is_empty() {
            anyhow::bail!("Tidak ada file yang terdownload");
        }
        Ok(files.into_iter().map(|f| f.display().to_string()).collect())
    })
    .await
}

#[tauri::command]
async fn detect(state: State<'_, AppState>, job_id: u64, input: String) -> R<Detection> {
    job(&state, job_id, Kind::Process, move |cfg| transcribe::detect(&cfg, &util::clean_path(&input))).await
}

#[derive(Serialize)]
struct TranscribeResult {
    srt: String,
    lang: lang::Lang,
    lines: usize,
}

#[tauri::command]
async fn transcribe(
    state: State<'_, AppState>,
    job_id: u64,
    input: String,
    lang: String,
    use_llm: bool,
    out: Option<String>,
) -> R<TranscribeResult> {
    job(&state, job_id, Kind::Process, move |cfg| {
        let opts = TranscribeOptions { lang, refine: use_llm, ..Default::default() };
        let o = transcribe::run(&cfg, &util::clean_path(&input), &opts, opt_path(out), None)?;
        Ok(TranscribeResult { srt: o.srt.display().to_string(), lang: o.lang, lines: o.lines })
    })
    .await
}

#[tauri::command]
async fn translate_srt(
    state: State<'_, AppState>,
    job_id: u64,
    input: String,
    to: String,
    from: Option<String>,
    out: Option<String>,
) -> R<String> {
    job(&state, job_id, Kind::Process, move |cfg| {
        let p = translate::run(&cfg, &util::clean_path(&input), &to, from.as_deref(), opt_path(out))?;
        Ok(p.display().to_string())
    })
    .await
}

#[tauri::command]
fn cancel(state: State<'_, AppState>, job_id: u64) {
    if let Some((_, c, _)) = state.jobs.lock().unwrap().get(&job_id) {
        c.store(true, Ordering::SeqCst);
    }
}

/// Jeda: berhenti tapi simpan bagian yang sudah diunduh (lanjut dengan download link yang sama).
#[tauri::command]
fn pause(state: State<'_, AppState>, job_id: u64) {
    if let Some((_, c, k)) = state.jobs.lock().unwrap().get(&job_id) {
        k.store(true, Ordering::SeqCst); // keep dulu, baru cancel
        c.store(true, Ordering::SeqCst);
    }
}

/// Self-test otomatis (hanya build debug, aktif bila VIDSUB_SELFTEST berisi JSON skenario).
#[tauri::command]
fn selftest_info() -> Option<String> {
    if cfg!(debug_assertions) { std::env::var("VIDSUB_SELFTEST").ok() } else { None }
}

#[tauri::command]
fn selftest_log(msg: String) {
    if cfg!(debug_assertions) {
        eprintln!("[selftest] {msg}");
    }
}

#[tauri::command]
fn resolve_lang(input: String) -> lang::Lang {
    lang::resolve(&input)
}

fn main() {
    let cfg = config::load();
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState { cfg: Mutex::new(cfg), jobs: Mutex::new(HashMap::new()) })
        .setup(|app| {
            let handle = app.handle().clone();
            report::set_sink(move |id, e| {
                #[derive(Serialize, Clone)]
                struct Payload {
                    job: Option<u64>,
                    #[serde(flatten)]
                    event: report::Event,
                }
                let _ = handle.emit("job", Payload { job: id, event: e });
            });
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.set_title("MultiTool");
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::DragDrop(tauri::DragDropEvent::Drop { paths, .. }) = event {
                let paths: Vec<String> = paths.iter().map(|p| p.display().to_string()).collect();
                let _ = window.emit("file-drop", paths);
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_settings,
            save_settings,
            list_models,
            languages,
            check_deps,
            media_info,
            srt_info,
            pick_folder,
            pick_file,
            save_srt,
            qr_preview,
            qr_save,
            pick_save_qr,
            open_path,
            probe,
            download,
            detect,
            transcribe,
            translate_srt,
            cancel,
            pause,
            resolve_lang,
            selftest_info,
            selftest_log,
        ])
        .run(tauri::generate_context!())
        .expect("gagal menjalankan MultiTool");
}
