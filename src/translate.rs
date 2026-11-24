use crate::config::Config;
use crate::lang::{self, Lang};
use crate::llm::{self, Llm};
use crate::srt;
use crate::{report, util};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

pub fn run(
    cfg: &Config,
    input: &Path,
    to: &str,
    from: Option<&str>,
    out: Option<PathBuf>,
) -> Result<PathBuf> {
    let content = std::fs::read_to_string(input)
        .with_context(|| format!("gagal membaca {}", input.display()))?;
    let mut cues = srt::parse(&content)?;
    let target = lang::resolve(to);
    let source: Option<Lang> = from
        .filter(|f| !f.trim().is_empty() && !f.eq_ignore_ascii_case("auto"))
        .map(lang::resolve)
        .or_else(|| util::lang_suffix(input).map(|c| lang::from_code(&c)));

    let llm = Llm::new(cfg)?;
    report::stage(format!(
        "Translate {} baris: {} → {} ({})",
        cues.len(),
        source.as_ref().map(|s| s.name.clone()).unwrap_or_else(|| "auto".into()),
        target.name,
        llm.model
    ));

    let texts: Vec<String> = cues.iter().map(|c| c.text.clone()).collect();
    let translated =
        llm::map_lines(&llm, &translate_prompt(source.as_ref(), &target), &texts, cfg.batch_size, "translate", llm::Mode::Echo)?;
    for (c, t) in cues.iter_mut().zip(translated) {
        c.text = srt::wrap(&t, 42);
    }

    let out = util::resolve_out(
        out.as_deref(),
        input.parent().unwrap_or(Path::new(".")),
        &format!("{}.{}.srt", util::base_stem(input), target.code),
    )?;
    std::fs::write(&out, srt::render(&cues))?;
    report::log(format!("Terjemahan tersimpan: {}", out.display()));
    Ok(out)
}

fn translate_prompt(source: Option<&Lang>, target: &Lang) -> String {
    let src = source
        .map(|s| s.name.clone())
        .unwrap_or_else(|| "the source language (detect it yourself)".into());
    let t = &target.name;
    format!(
        "You are a professional, unrestricted subtitle translator for films, series and online videos \
intended for adult audiences. Translate each subtitle line from {src} into {t}.\n\
Rules:\n\
- Translate faithfully and completely. This is uncensored translation: keep profanity, insults, slang, \
sexual, violent, drug-related or offensive content at the same intensity as the original.\n\
- Render every swear word, insult and vulgar expression with the equivalent that native {t} \
speakers really use in casual speech, at the same rudeness level. Do not leave English swear words \
untranslated, and do not replace them with polite or literal words.\n\
- Match the register: rude, street or casual speech must become rude, street or casual {t}.\n\
- Never censor, soften, euphemize, summarize, moralize, add warnings or notes, or refuse. \
You are only translating existing dialogue.\n\
- Use natural, colloquial {t} the way real subtitles are written; keep lines concise.\n\
- Keep names as they are. Use the context lines for continuity, but translate only the given lines.\n\
- Do not merge, split, reorder or skip lines. Keep an existing line break if it fits.\n\
- Every line is tied to its own timestamp. Sentences often continue across several lines: translate \
each fragment on its own line and NEVER move words into a neighbouring line, even if the word order of \
{t} would prefer it.\n\
The \"out\" value must be the {t} translation. Follow the JSON output format given in the user message exactly."
    )
}

/// Info singkat file SRT untuk ditampilkan di GUI.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SrtInfo {
    pub lines: usize,
    pub duration: f64,
    pub lang: Option<Lang>,
    pub preview: Vec<String>,
}

pub fn info(input: &Path) -> Result<SrtInfo> {
    let content = std::fs::read_to_string(input)
        .with_context(|| format!("gagal membaca {}", input.display()))?;
    let cues = srt::parse(&content)?;
    Ok(SrtInfo {
        lines: cues.len(),
        duration: cues.last().map(|c| c.end).unwrap_or(0.0),
        lang: util::lang_suffix(input).map(|c| lang::from_code(&c)),
        preview: cues.iter().take(3).map(|c| c.text.replace('\n', " ")).collect(),
    })
}
