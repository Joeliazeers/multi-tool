use crate::picker::{self, Kind};
use multitool::config::Config;
use multitool::download::{self, DownloadArgs, Quality};
use multitool::lang::{self, LANGS, Lang};
use multitool::transcribe::{self, TranscribeOptions};
use multitool::translate;
use multitool::util;
use anyhow::{Result, bail};
use dialoguer::{Confirm, Input, Select, theme::ColorfulTheme};
use std::path::{Path, PathBuf};

#[derive(Default)]
struct State {
    last_video: Option<PathBuf>,
    last_srt: Option<PathBuf>,
    last_lang: Option<Lang>,
    last_dl_dir: Option<PathBuf>,
}

pub fn run(cfg: &Config) -> Result<()> {
    let theme = ColorfulTheme::default();
    let mut st = State::default();
    println!("=== MultiTool — download · transcribe · translate ===\n");
    loop {
        let items = [
            "⬇  Download video",
            "📝 Transcribe video → SRT",
            "🌐 Translate SRT",
            "⚡ Semua sekaligus (download → transcribe → translate)",
            "⚙  Lihat konfigurasi",
            "🚪 Keluar",
        ];
        let choice = Select::with_theme(&theme)
            .with_prompt("Pilih menu")
            .items(items)
            .default(0)
            .interact()?;
        println!();
        let r = match choice {
            0 => do_download(cfg, &theme, &mut st).map(|_| ()),
            1 => do_transcribe(cfg, &theme, &mut st),
            2 => do_translate(cfg, &theme, &mut st),
            3 => do_all(cfg, &theme, &mut st),
            4 => {
                cfg.print();
                Ok(())
            }
            _ => return Ok(()),
        };
        if let Err(e) = r {
            eprintln!("\n✖ Error: {e:#}");
        }
        println!();
    }
}

fn opt(s: String) -> Option<String> {
    let s = s.trim().to_string();
    (!s.is_empty()).then_some(s)
}

fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

fn parent_of(p: &Path) -> PathBuf {
    p.parent().map(Path::to_path_buf).unwrap_or_else(home)
}

// ---------------------------------------------------------------- lokasi simpan

/// Pilih folder. Default ditampilkan sebagai opsi pertama.
fn ask_folder(theme: &ColorfulTheme, prompt: &str, default: &Path) -> Result<PathBuf> {
    let gui = picker::available();
    let mut items = vec![format!("📁 {}", default.display())];
    if gui {
        items.push("🗂  Pilih folder lain… (dialog)".into());
    }
    items.push("⌨  Ketik path folder".into());
    let i = Select::with_theme(theme).with_prompt(prompt).items(&items).default(0).interact()?;
    match (i, gui) {
        (0, _) => Ok(default.to_path_buf()),
        (1, true) => match picker::folder(prompt, default) {
            Some(p) => Ok(p),
            None => {
                println!("  (dialog dibatalkan, pakai default)");
                Ok(default.to_path_buf())
            }
        },
        _ => {
            let s: String = Input::with_theme(theme)
                .with_prompt("Path folder")
                .default(default.display().to_string())
                .interact_text()?;
            Ok(util::clean_path(&s))
        }
    }
}

/// Lokasi simpan SRT: folder (nama file otomatis) atau file tertentu.
/// Hasil dipakai sebagai `out` untuk transcribe/translate.
fn ask_srt_out(theme: &ColorfulTheme, prompt: &str, default_dir: &Path, suggested_name: &str) -> Result<PathBuf> {
    let gui = picker::available();
    let mut items = vec![format!("📁 {} (nama otomatis)", default_dir.display())];
    if gui {
        items.push("🗂  Pilih folder lain… (dialog)".into());
        items.push("💾 Simpan sebagai file… (dialog)".into());
    }
    items.push("⌨  Ketik path (folder atau file .srt)".into());
    let i = Select::with_theme(theme).with_prompt(prompt).items(&items).default(0).interact()?;
    let cancelled = || {
        println!("  (dialog dibatalkan, pakai default)");
        default_dir.to_path_buf()
    };
    Ok(match (i, gui) {
        (0, _) => default_dir.to_path_buf(),
        (1, true) => picker::folder(prompt, default_dir).unwrap_or_else(cancelled),
        (2, true) => picker::save_file(prompt, &default_dir.join(suggested_name)).unwrap_or_else(cancelled),
        _ => {
            let s: String = Input::with_theme(theme)
                .with_prompt("Path folder atau file .srt")
                .default(default_dir.display().to_string())
                .interact_text()?;
            util::clean_path(&s)
        }
    })
}

/// Pilih file input (video atau SRT).
fn ask_input(theme: &ColorfulTheme, prompt: &str, last: Option<&PathBuf>, kind: Kind) -> Result<PathBuf> {
    let gui = picker::available();
    let last = last.filter(|p| p.is_file());
    let mut items = Vec::new();
    if let Some(l) = last {
        items.push(format!("↩  Terakhir: {}", l.display()));
    }
    if gui {
        items.push("🗂  Pilih file… (dialog)".into());
    }
    items.push("⌨  Ketik / tempel path".into());
    let mut i = Select::with_theme(theme).with_prompt(prompt).items(&items).default(0).interact()?;
    if let Some(l) = last {
        if i == 0 {
            return Ok(l.clone());
        }
        i -= 1;
    }
    if gui && i == 0 {
        let start = last.map(|l| parent_of(l)).unwrap_or_else(home);
        if let Some(p) = picker::open_file(prompt, &start, kind) {
            return Ok(p);
        }
        println!("  (dialog dibatalkan, ketik path saja)");
    }
    let s = Input::<String>::with_theme(theme)
        .with_prompt("Path file")
        .validate_with(|s: &String| if util::clean_path(s).is_file() { Ok(()) } else { Err("file tidak ditemukan") })
        .interact_text()?;
    Ok(util::clean_path(&s))
}

// ---------------------------------------------------------------- download

fn ask_download(cfg: &Config, theme: &ColorfulTheme, st: &State) -> Result<DownloadArgs> {
    let url: String = Input::with_theme(theme)
        .with_prompt("Link video")
        .validate_with(|s: &String| if s.trim().is_empty() { Err("link tidak boleh kosong") } else { Ok(()) })
        .interact_text()?;
    let referer: String = Input::with_theme(theme)
        .with_prompt("Referer (Enter = tanpa referer)")
        .allow_empty(true)
        .interact_text()?;
    let qualities = [
        ("Terbaik", Quality::Best),
        ("Maks 1080p", Quality::P1080),
        ("Maks 720p", Quality::P720),
        ("Maks 480p", Quality::P480),
        ("Maks 360p", Quality::P360),
        ("Audio saja (cukup untuk transcribe)", Quality::Audio),
    ];
    let qi = Select::with_theme(theme)
        .with_prompt("Kualitas")
        .items(qualities.iter().map(|q| q.0))
        .default(0)
        .interact()?;
    let default_dir = st.last_dl_dir.clone().unwrap_or_else(|| cfg.output_dir());
    let dir = ask_folder(theme, "Simpan video ke folder", &default_dir)?;
    let mut a = DownloadArgs {
        url: url.trim().into(),
        referer: opt(referer),
        output_dir: Some(dir),
        quality: qualities[qi].1,
        ..Default::default()
    };

    if Confirm::with_theme(theme).with_prompt("Opsi lanjutan? (cookies, format, playlist)").default(false).interact()? {
        a.cookies_from_browser = opt(Input::with_theme(theme).with_prompt("Cookies dari browser (firefox/chrome/brave, Enter = tidak)").allow_empty(true).interact_text()?);
        a.format = opt(Input::with_theme(theme).with_prompt("Format yt-dlp manual (Enter = sesuai kualitas)").allow_empty(true).interact_text()?);
        a.playlist = Confirm::with_theme(theme).with_prompt("Download seluruh playlist?").default(false).interact()?;
    }
    Ok(a)
}

fn run_download(cfg: &Config, st: &mut State, a: &DownloadArgs) -> Result<Vec<PathBuf>> {
    let files = download::run(cfg, a)?;
    st.last_dl_dir = a.output_dir.clone();
    if let Some(f) = files.last() {
        st.last_video = Some(f.clone());
    }
    Ok(files)
}

fn do_download(cfg: &Config, theme: &ColorfulTheme, st: &mut State) -> Result<Vec<PathBuf>> {
    let a = ask_download(cfg, theme, st)?;
    run_download(cfg, st, &a)
}

// ---------------------------------------------------------------- bahasa

/// Pilih bahasa. `allow_auto` menambah opsi "Auto". None = auto.
fn pick_lang(theme: &ColorfulTheme, prompt: &str, allow_auto: bool, default_code: &str) -> Result<Option<Lang>> {
    let mut items: Vec<String> = Vec::new();
    if allow_auto {
        items.push("🔍 Auto — whisper menebak bahasa".into());
    }
    let offset = items.len();
    items.extend(LANGS.iter().map(|l| format!("{} / {} ({})", l.1, l.2, l.0)));
    items.push("✏  Lainnya (ketik manual)".into());
    let default = if allow_auto { 0 } else { LANGS.iter().position(|l| l.0 == default_code).unwrap_or(0) };
    let i = Select::with_theme(theme)
        .with_prompt(prompt)
        .items(&items)
        .default(default)
        .max_length(12)
        .interact()?;
    if allow_auto && i == 0 {
        return Ok(None);
    }
    if i == items.len() - 1 {
        let s: String = Input::with_theme(theme)
            .with_prompt("Kode atau nama bahasa (mis. 'sw', 'Bengali', 'Indonesian slang Jakarta')")
            .interact_text()?;
        return Ok(Some(lang::resolve(&s)));
    }
    let l = LANGS[i - offset];
    Ok(Some(Lang { code: l.0.into(), name: l.1.into() }))
}

fn ask_transcribe_opts(theme: &ColorfulTheme) -> Result<TranscribeOptions> {
    let lang = pick_lang(theme, "Bahasa di video", true, "")?;
    let mut o = TranscribeOptions { lang: lang.map(|l| l.code).unwrap_or_else(|| "auto".into()), ..Default::default() };
    o.refine = Confirm::with_theme(theme)
        .with_prompt("Rapikan & buat SRT dengan LLM?")
        .default(false)
        .interact()?;
    Ok(o)
}

// ---------------------------------------------------------------- transcribe

fn transcribe_with(
    cfg: &Config,
    theme: &ColorfulTheme,
    st: &mut State,
    video: PathBuf,
    o: &TranscribeOptions,
    out: Option<PathBuf>,
) -> Result<()> {
    let confirm = |l: &Lang| -> Result<Lang> {
        if o.lang != "auto" {
            return Ok(l.clone());
        }
        if Confirm::with_theme(theme).with_prompt(format!("Pakai bahasa {l}?")).default(true).interact()? {
            Ok(l.clone())
        } else {
            Ok(pick_lang(theme, "Pilih bahasa", false, &l.code)?.unwrap())
        }
    };
    let res = transcribe::run(cfg, &video, o, out, Some(&confirm))?;
    st.last_video = Some(video);
    st.last_srt = Some(res.srt);
    st.last_lang = Some(res.lang);
    Ok(())
}

fn do_transcribe(cfg: &Config, theme: &ColorfulTheme, st: &mut State) -> Result<()> {
    let video = ask_input(theme, "Video/audio yang mau ditranscribe", st.last_video.as_ref(), Kind::Video)?;
    let o = ask_transcribe_opts(theme)?;
    let name_lang = if o.lang == "auto" { "auto".to_string() } else { o.lang.clone() };
    let out = ask_srt_out(
        theme,
        "Simpan SRT ke",
        &parent_of(&video),
        &format!("{}.{name_lang}.srt", util::base_stem(&video)),
    )?;
    transcribe_with(cfg, theme, st, video, &o, Some(out))
}

// ---------------------------------------------------------------- translate

fn ask_target(theme: &ColorfulTheme) -> Result<Lang> {
    Ok(pick_lang(theme, "Translate ke bahasa", false, "id")?.unwrap())
}

fn do_translate(cfg: &Config, theme: &ColorfulTheme, st: &mut State) -> Result<()> {
    let srt = ask_input(theme, "File SRT yang mau ditranslate", st.last_srt.as_ref(), Kind::Srt)?;
    let target = ask_target(theme)?;
    let out = ask_srt_out(
        theme,
        "Simpan terjemahan ke",
        &parent_of(&srt),
        &format!("{}.{}.srt", util::base_stem(&srt), target.code),
    )?;
    let from = st.last_srt.as_ref().filter(|p| **p == srt).and(st.last_lang.as_ref()).map(|l| l.code.clone());
    let res = translate::run(cfg, &srt, &target.name, from.as_deref(), Some(out))?;
    st.last_srt = Some(res);
    Ok(())
}

// ---------------------------------------------------------------- semua

fn do_all(cfg: &Config, theme: &ColorfulTheme, st: &mut State) -> Result<()> {
    let a = ask_download(cfg, theme, st)?;
    let o = ask_transcribe_opts(theme)?;
    let target = if Confirm::with_theme(theme).with_prompt("Translate juga?").default(true).interact()? {
        Some(ask_target(theme)?)
    } else {
        None
    };
    let video_dir = a.output_dir.clone().unwrap_or_else(|| cfg.output_dir());
    let srt_dir = ask_folder(theme, "Simpan file SRT ke folder", &video_dir)?;

    let files = run_download(cfg, st, &a)?;
    if files.is_empty() {
        bail!("Tidak ada file yang terdownload");
    }
    for f in files {
        transcribe_with(cfg, theme, st, f, &o, Some(srt_dir.clone()))?;
        if let (Some(t), Some(srt)) = (&target, st.last_srt.clone()) {
            let from = st.last_lang.as_ref().map(|l| l.code.clone());
            st.last_srt = Some(translate::run(cfg, &srt, &t.name, from.as_deref(), Some(srt_dir.clone()))?);
        }
    }
    Ok(())
}
