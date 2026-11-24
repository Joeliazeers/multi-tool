mod menu;
mod picker;

use multitool::{config, download, qr, transcribe, translate, util};

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

/// MultiTool: download video (yt-dlp), transcribe ke SRT, translate SRT dengan LLM, buat QR code.
/// Jalankan tanpa argumen untuk menu interaktif.
#[derive(Parser)]
#[command(name = "multitool", version)]
struct Cli {
    #[command(flatten)]
    cfg: config::Config,

    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Download video dengan yt-dlp
    Download(download::DownloadArgs),

    /// Transcribe video/audio → SRT
    Transcribe {
        /// File video/audio
        input: PathBuf,
        #[command(flatten)]
        opts: transcribe::TranscribeOptions,
        /// Lokasi simpan SRT: folder atau path file .srt (default: folder video)
        #[arg(short = 'o', long)]
        out: Option<PathBuf>,
    },

    /// Translate file SRT dengan LLM (tanpa sensor)
    Translate {
        /// File SRT
        input: PathBuf,
        /// Bahasa tujuan: kode/nama, mis. id, en, "Indonesian (gaul Jakarta)"
        #[arg(short = 't', long)]
        to: String,
        /// Bahasa sumber (default: dari nama file, atau auto)
        #[arg(long)]
        from: Option<String>,
        /// Lokasi simpan hasil: folder atau path file .srt (default: folder SRT asal)
        #[arg(short = 'o', long)]
        out: Option<PathBuf>,
    },

    /// Download → transcribe → (opsional) translate sekaligus
    Auto {
        #[command(flatten)]
        dl: download::DownloadArgs,
        #[command(flatten)]
        opts: transcribe::TranscribeOptions,
        /// Bahasa tujuan terjemahan (kosong = tanpa translate)
        #[arg(short = 't', long)]
        to: Option<String>,
        /// Folder simpan file SRT (default: sama dengan folder video)
        #[arg(short = 's', long)]
        srt_dir: Option<PathBuf>,
    },

    /// Buat QR code dari link/teks, simpan sebagai PNG/JPG/SVG/WebP/BMP
    Qr {
        /// Link atau teks yang dijadikan QR
        text: String,
        /// File hasil; format dari ekstensi (png, jpg, svg, webp, bmp)
        #[arg(short = 'o', long, default_value = "qr.png")]
        out: PathBuf,
        #[command(flatten)]
        opts: qr::QrOptions,
    },

    /// Tampilkan konfigurasi aktif
    Config,
}

fn main() -> Result<()> {
    config::load_env();
    let cli = Cli::parse();
    let cfg = &cli.cfg;
    match cli.cmd {
        None => menu::run(cfg),
        Some(Cmd::Config) => {
            cfg.print();
            Ok(())
        }
        Some(Cmd::Qr { text, out, opts }) => {
            let path = qr::save(&text, &opts, &util::clean_path(&out.to_string_lossy()))?;
            println!("✔ QR disimpan: {}", path.display());
            Ok(())
        }
        Some(Cmd::Download(a)) => download::run(cfg, &a).map(|_| ()),
        Some(Cmd::Transcribe { input, opts, out }) => {
            transcribe::run(cfg, &util::clean_path(&input.to_string_lossy()), &opts, out, None).map(|_| ())
        }
        Some(Cmd::Translate { input, to, from, out }) => {
            translate::run(cfg, &util::clean_path(&input.to_string_lossy()), &to, from.as_deref(), out).map(|_| ())
        }
        Some(Cmd::Auto { dl, opts, to, srt_dir }) => {
            let files = download::run(cfg, &dl)?;
            if files.is_empty() {
                anyhow::bail!("Tidak ada file yang terdownload");
            }
            for f in files {
                let t = transcribe::run(cfg, &f, &opts, srt_dir.clone(), None)?;
                if let Some(to) = &to {
                    translate::run(cfg, &t.srt, to, Some(&t.lang.code), srt_dir.clone())?;
                }
            }
            Ok(())
        }
    }
}
