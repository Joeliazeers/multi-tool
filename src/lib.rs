//! Inti MultiTool: download (yt-dlp), transcribe (whisper + LLM), translate SRT (LLM), QR code.
//! Dipakai oleh CLI (`multitool`) dan GUI (`multitool-gui`).
pub mod asr;
pub mod audio;
pub mod config;
pub mod download;
pub mod fetch;
pub mod lang;
pub mod llm;
pub mod qr;
pub mod report;
pub mod srt;
pub mod transcribe;
pub mod translate;
pub mod util;
