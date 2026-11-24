/// (kode ISO / Whisper, nama Inggris, nama Indonesia)
pub const LANGS: &[(&str, &str, &str)] = &[
    ("id", "Indonesian", "Indonesia"),
    ("en", "English", "Inggris"),
    ("ja", "Japanese", "Jepang"),
    ("ko", "Korean", "Korea"),
    ("zh", "Chinese (Simplified)", "Mandarin/Tionghoa"),
    ("th", "Thai", "Thailand"),
    ("vi", "Vietnamese", "Vietnam"),
    ("ms", "Malay", "Melayu"),
    ("tl", "Tagalog", "Tagalog"),
    ("jw", "Javanese", "Jawa"),
    ("su", "Sundanese", "Sunda"),
    ("hi", "Hindi", "Hindi"),
    ("ar", "Arabic", "Arab"),
    ("ru", "Russian", "Rusia"),
    ("es", "Spanish", "Spanyol"),
    ("pt", "Portuguese", "Portugis"),
    ("fr", "French", "Prancis"),
    ("de", "German", "Jerman"),
    ("it", "Italian", "Italia"),
    ("tr", "Turkish", "Turki"),
    ("nl", "Dutch", "Belanda"),
    ("pl", "Polish", "Polandia"),
    ("uk", "Ukrainian", "Ukraina"),
    ("fa", "Persian", "Persia"),
];

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Lang {
    /// kode untuk Whisper & nama file
    pub code: String,
    /// nama untuk prompt LLM
    pub name: String,
}

impl std::fmt::Display for Lang {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.name, self.code)
    }
}

pub fn from_code(code: &str) -> Lang {
    let code = code.trim().to_lowercase();
    let code = if code == "jv" { "jw".to_string() } else { code };
    let name = LANGS
        .iter()
        .find(|l| l.0 == code)
        .map(|l| l.1.to_string())
        .unwrap_or_else(|| code.clone());
    Lang { code, name }
}

/// Terima kode ("ja"), nama Inggris ("Japanese"), nama Indonesia ("Jepang"),
/// atau deskripsi bebas ("Indonesian (slang Jakarta)") untuk target terjemahan.
/// "Chinese (Simplified)" -> "chinese"
fn base_name(name: &str) -> String {
    name.split(" (").next().unwrap_or(name).trim().to_lowercase()
}

pub fn resolve(input: &str) -> Lang {
    let t = input.trim();
    let lower = t.to_lowercase();
    if let Some(l) = LANGS.iter().find(|l| {
        l.0 == lower
            || l.1.eq_ignore_ascii_case(t)
            || base_name(l.1) == lower
            || l.2.eq_ignore_ascii_case(t)
            || l.2.to_lowercase().split('/').any(|p| p == lower)
            || format!("bahasa {}", l.2.to_lowercase()) == lower
    }) {
        return Lang { code: l.0.into(), name: l.1.into() };
    }
    if lower == "jv" {
        return from_code("jw");
    }
    if (2..=3).contains(&lower.len()) && lower.chars().all(|c| c.is_ascii_lowercase()) {
        return from_code(&lower);
    }
    // deskripsi bebas: tebak kode dari kata pertama yang cocok, kalau tidak pakai slug
    let code = LANGS
        .iter()
        .find(|l| lower.contains(&base_name(l.1)) || lower.contains(&l.2.to_lowercase()))
        .map(|l| l.0.to_string())
        .unwrap_or_else(|| {
            lower
                .chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .take(8)
                .collect::<String>()
        });
    Lang { code: if code.is_empty() { "xx".into() } else { code }, name: t.to_string() }
}

/// "english" / "Japanese" -> kode (untuk respons API whisper yang memberi nama bahasa).
pub fn code_from_name(name: &str) -> Option<String> {
    let n = name.trim().to_lowercase();
    if is_valid_code(&n) {
        return Some(n);
    }
    LANGS.iter().find(|l| l.1.to_lowercase() == n || base_name(l.1) == n).map(|l| l.0.to_string())
}

pub fn is_valid_code(code: &str) -> bool {
    (2..=3).contains(&code.len()) && code.chars().all(|c| c.is_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves() {
        assert_eq!(resolve("Jepang").code, "ja");
        assert_eq!(resolve("english").name, "English");
        assert_eq!(resolve("bahasa indonesia").code, "id");
        let free = resolve("Indonesian (slang Jakarta)");
        assert_eq!(free.code, "id");
        assert_eq!(free.name, "Indonesian (slang Jakarta)");
        assert_eq!(resolve("sw").code, "sw");
        assert_eq!(resolve("Chinese").code, "zh");
        assert_eq!(resolve("Chinese (Simplified)").code, "zh");
        assert_eq!(resolve("chinese gaul").code, "zh");
        assert_eq!(code_from_name("chinese").as_deref(), Some("zh"));
    }
}
