use crate::asr;
use crate::audio;
use crate::config::Config;
use crate::lang::{self, Lang};
use crate::llm::{self, Llm};
use crate::srt::{self, Cue};
use crate::{report, util};
use anyhow::{Result, bail};
use clap::Args;
use std::path::{Path, PathBuf};

#[derive(Args, Debug, Clone)]
pub struct TranscribeOptions {
    /// Bahasa audio: "auto" (whisper menebak) atau kode/nama (ja, en, Jepang, ...)
    #[arg(short = 'l', long = "lang", default_value = "auto")]
    pub lang: String,

    /// Rapikan hasil whisper dengan LLM (perbaiki salah dengar/tanda baca). Default mati:
    /// model kecil (mis. 8B) bisa mengubah arti atau memperhalus kata kasar.
    #[arg(long)]
    pub refine: bool,

    /// Prompt awal untuk whisper (istilah, nama, gaya bahasa)
    #[arg(long)]
    pub prompt: Option<String>,

    /// Matikan filter audio (highpass/normalisasi) sebelum whisper
    #[arg(long)]
    pub no_audio_filter: bool,
}

impl Default for TranscribeOptions {
    fn default() -> Self {
        Self { lang: "auto".into(), refine: false, prompt: None, no_audio_filter: false }
    }
}

/// Callback untuk konfirmasi/ubah bahasa hasil tebakan (dipakai mode interaktif).
pub type ConfirmLang<'a> = &'a dyn Fn(&Lang) -> Result<Lang>;

pub struct Output {
    pub srt: PathBuf,
    pub lang: Lang,
    pub lines: usize,
}

/// Hasil deteksi bahasa (oleh whisper).
#[derive(Debug, Clone, serde::Serialize)]
pub struct Detection {
    pub lang: Lang,
    /// Probabilitas 0..1 (None bila backend tidak memberi angka)
    pub confidence: Option<f64>,
    /// Kandidat lain beserta probabilitasnya, urut menurun
    pub candidates: Vec<(Lang, f64)>,
}

/// Deteksi bahasa langsung dari file video/audio (tanpa transkripsi).
pub fn detect(cfg: &Config, input: &Path) -> Result<Detection> {
    if !input.exists() {
        bail!("File tidak ditemukan: {}", input.display());
    }
    let tmp = tempfile::tempdir()?;
    let dur = audio::duration(input)?;
    detect_language(cfg, input, dur, tmp.path())
}

pub fn run(
    cfg: &Config,
    input: &Path,
    opts: &TranscribeOptions,
    out: Option<PathBuf>,
    confirm: Option<ConfirmLang>,
) -> Result<Output> {
    if !input.exists() {
        bail!("File tidak ditemukan: {}", input.display());
    }
    let llm = Llm::new(cfg)?;
    if opts.refine {
        llm.warmup(); // muat model LLM paralel dengan whisper
    }
    let tmp = tempfile::tempdir()?;
    let wav = tmp.path().join("audio.wav");

    report::stage(format!("Ekstrak audio dari {}", input.display()));
    report::progress(None, "ffmpeg");
    audio::extract_wav(input, &wav, !opts.no_audio_filter)?;
    let dur = audio::duration(&wav)?;

    // 1) Tentukan bahasa
    let mut lang = if opts.lang.trim().eq_ignore_ascii_case("auto") {
        detect_language(cfg, &wav, dur, tmp.path())?.lang
    } else {
        let l = lang::resolve(&opts.lang);
        if !lang::is_valid_code(&l.code) {
            bail!("Bahasa tidak dikenali: {}", opts.lang);
        }
        l
    };
    if let Some(confirm) = confirm {
        lang = confirm(&lang)?;
    }
    report::log(format!("Bahasa: {lang}"));

    // 2) Transkripsi penuh dengan bahasa yang dipilih
    report::stage(format!("Transkripsi {} ({:.0} menit audio)", lang.name, (dur / 60.0).max(1.0)));
    report::progress(Some(0.0), "memuat model whisper...");
    let res = asr::transcribe(cfg, &wav, Some(&lang.code), opts.prompt.as_deref(), false)?;
    if res.segments.is_empty() {
        bail!("Tidak ada ucapan yang terdeteksi.");
    }
    let mut cues: Vec<Cue> = res
        .segments
        .iter()
        .map(|s| Cue { start: s.start, end: s.end, text: s.text.trim().to_string() })
        .collect();

    // 3) LLM merapikan & memastikan SRT dalam bahasa yang dipilih
    if opts.refine {
        report::stage(format!("Merapikan subtitle dengan LLM ({})", llm.model));
        let texts: Vec<String> = cues.iter().map(|c| c.text.clone()).collect();
        let fixed = llm::map_lines(&llm, &refine_prompt(&lang), &texts, cfg.batch_size, "rapikan", llm::Mode::Plain)?;
        let mut rejected = 0;
        for (i, t) in fixed.into_iter().enumerate() {
            // Tolak koreksi yang terlalu jauh dari aslinya, atau yang mengambil kata dari
            // baris tetangga (LLM suka "melengkapi kalimat" -> timing tidak sinkron).
            let around: Vec<&str> = (i.saturating_sub(2)..(i + 3).min(texts.len()))
                .filter(|&j| j != i)
                .map(|j| texts[j].as_str())
                .collect();
            if util::similar_enough(&texts[i], &t) && !util::borrows_from_neighbors(&texts[i], &t, &around) {
                cues[i].text = t;
            } else {
                rejected += 1;
            }
        }
        if rejected > 0 {
            report::log(format!("{rejected} koreksi LLM ditolak karena terlalu berbeda dari audio (teks whisper dipakai)"));
        }
    }
    for c in &mut cues {
        c.text = srt::wrap(&c.text, 42);
    }

    let out = util::resolve_out(
        out.as_deref(),
        input.parent().unwrap_or(Path::new(".")),
        &format!("{}.{}.srt", util::base_stem(input), lang.code),
    )?;
    std::fs::write(&out, srt::render(&cues))?;
    report::log(format!("SRT tersimpan: {} ({} baris)", out.display(), cues.len()));
    Ok(Output { srt: out, lang, lines: cues.len() })
}

fn detect_language(cfg: &Config, input: &Path, dur: f64, tmp: &Path) -> Result<Detection> {
    // Sampel 3 menit (lewati 30 detik awal kalau video panjang: biasanya intro/musik).
    // Whisper memeriksa beberapa jendela 30 detik di dalamnya dan melewati bagian tanpa suara.
    let start = if dur > 300.0 { 30.0 } else { 0.0 };
    let len = 180.0f64.min(dur - start);
    let sample = tmp.join("sample.wav");
    audio::cut(input, &sample, start, len, false)?;

    report::stage("Mendeteksi bahasa (whisper)");
    report::progress(None, "whisper mendengarkan sampel...");
    let d = asr::detect(cfg, &sample)?;
    if d.speech_seconds.is_some_and(|s| s < 2.0) {
        bail!("Tidak ada ucapan di {len:.0} detik sampel, bahasa tidak bisa ditebak. Pilih bahasa secara manual.");
    }
    let code = d.language.trim().to_lowercase();
    if !lang::is_valid_code(&code) {
        bail!("Bahasa tidak bisa dideteksi, pilih bahasa secara manual (--lang)");
    }
    let confidence = d.language_probability.is_finite().then_some(d.language_probability);
    let candidates: Vec<(Lang, f64)> = d
        .candidates
        .iter()
        .filter(|(c, p)| *p >= 0.01 && lang::is_valid_code(c))
        .map(|(c, p)| (lang::from_code(c), *p))
        .collect();
    let l = lang::from_code(&code);
    let alt: Vec<String> = candidates.iter().skip(1).take(2).map(|(l, p)| format!("{} {:.0}%", l.name, p * 100.0)).collect();
    report::log(format!(
        "Whisper: {l}{}{}",
        confidence.map(|c| format!(" — keyakinan {:.0}%", c * 100.0)).unwrap_or_default(),
        if alt.is_empty() { String::new() } else { format!(" (lainnya: {})", alt.join(", ")) }
    ));
    if confidence.is_some_and(|c| c < 0.6) {
        report::warn("keyakinan rendah — sebaiknya cek atau pilih bahasa manual");
    }
    Ok(Detection { lang: l, confidence, candidates })
}

fn refine_prompt(lang: &Lang) -> String {
    let n = &lang.name;
    format!(
        "You are an expert {n} subtitle editor. You receive raw automatic speech recognition (ASR) lines \
from a video as a JSON object (id -> text). For each line:\n\
- Fix misrecognized words, spelling, punctuation and capitalization so it reads as natural, correct {n}.\n\
- The output text MUST be written in {n}. If a line is in another language or script, render it in {n}.\n\
- Keep the original meaning and tone. Do not summarize, merge, split, reorder or drop lines.\n\
- Every line is tied to its own timestamp. Sentences often continue across several lines: that is \
expected. NEVER move, copy or add words from neighbouring lines; a line that starts or ends mid-sentence \
must stay a fragment.\n\
- Keep profanity, slang, sexual or violent wording exactly as spoken. Never censor, soften or add comments.\n\
- If a line is already correct, return it unchanged.\n\
Follow the JSON output format given in the user message exactly."
    )
}

#[cfg(test)]
mod tests {
    use crate::util::similar_enough;

    #[test]
    fn guard() {
        assert!(similar_enough("President Pitta, Mr. Vice President", "President Pitzer, Mr. Vice President,"));
        assert!(!similar_enough(
            "assure you that my first lecture will be very brief.",
            "I am delighted to be here, and I'm particularly delighted to be here on this occasion."
        ));
        assert!(!similar_enough(
            "and I will",
            "I appreciate your president having made me an honorary visiting professor, and I will"
        ));
    }
}
