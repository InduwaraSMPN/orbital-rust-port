//! Localization port of `Loc.cs` + the 10 `Strings.*.xaml` catalogs.
//! Catalogs live in `locales/<lang>.json` (converted, UTF-8), compiled in via
//! `include_str!`. `get` falls back to the key itself; `get_fmt` reproduces
//! `string.Format` `{0}` placeholder substitution.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};

const LANGUAGES: &[&str] = &["en", "ko", "ja", "zh", "es", "fr", "de", "pt", "ru", "it"];

fn locale_for(lang: &str) -> &'static str {
    match lang {
        "ko" => include_str!("../locales/ko.json"),
        "ja" => include_str!("../locales/ja.json"),
        "zh" => include_str!("../locales/zh.json"),
        "es" => include_str!("../locales/es.json"),
        "fr" => include_str!("../locales/fr.json"),
        "de" => include_str!("../locales/de.json"),
        "pt" => include_str!("../locales/pt.json"),
        "ru" => include_str!("../locales/ru.json"),
        "it" => include_str!("../locales/it.json"),
        _ => include_str!("../locales/en.json"),
    }
}

type Catalog = serde_json::Map<String, serde_json::Value>;

fn catalog(lang: &str) -> Catalog {
    serde_json::from_str(locale_for(lang)).unwrap_or_default()
}

/// Parsed catalogs, one slot per entry of `LANGUAGES` (the slot index is the
/// language index). `OnceLock` keeps the parse lazy without a lock on read.
static CATALOGS: [OnceLock<Catalog>; LANGUAGES.len()] =
    [const { OnceLock::new() }; LANGUAGES.len()];

fn catalog_at(index: usize) -> &'static Catalog {
    let index = index.min(LANGUAGES.len() - 1);
    CATALOGS[index].get_or_init(|| catalog(LANGUAGES[index]))
}

/// C# `Loc.Get(key)` — key fallback when missing.
pub fn get(key: &str) -> String {
    match catalog_at(current_index()).get(key) {
        // Only string resources count, mirroring `TryFindResource(key) is string`.
        Some(serde_json::Value::String(value)) => value.clone(),
        _ => key.to_string(),
    }
}

/// `string.Format(Loc.Get(key), args...)` — replaces `{0}`, `{1}`, … in order.
pub fn get_fmt(key: &str, args: &[&str]) -> String {
    let mut result = get(key);
    for (index, arg) in args.iter().enumerate() {
        let placeholder = format!("{{{index}}}");
        result = result.replace(&placeholder, arg);
    }
    result
}

/// C# `App.ApplyLanguage` — unsupported codes fall back to `en`.
pub fn set_language(code: &str) {
    CURRENT.store(index_of(code) as u8, Ordering::Relaxed);
}

pub fn current_language() -> &'static str {
    LANGUAGES[current_index()]
}

pub fn supported_languages() -> &'static [&'static str] {
    LANGUAGES
}

static CURRENT: AtomicU8 = AtomicU8::new(0);

fn index_of(code: &str) -> usize {
    LANGUAGES.iter().position(|l| *l == code).unwrap_or(0)
}

fn current_index() -> usize {
    CURRENT.load(Ordering::Relaxed) as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// `CURRENT` is process-global, so the language-dependent tests must not
    /// interleave with each other.
    static LANGUAGE_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn get_returns_the_catalog_value_or_the_key_itself() {
        let _guard = LANGUAGE_LOCK.lock().expect("language lock");
        set_language("en");
        assert_eq!(get("Str_Save"), "Save");
        assert_eq!(get("Str_AppTitle"), "Orbital");
        assert_eq!(get("Str_ThereIsNoSuchKey"), "Str_ThereIsNoSuchKey");
        assert_eq!(get(""), "");
    }

    #[test]
    fn get_fmt_substitutes_placeholders_in_order() {
        let _guard = LANGUAGE_LOCK.lock().expect("language lock");
        set_language("en");
        assert_eq!(
            get_fmt("Str_DeleteActionConfirm", &["My Action"]),
            "Delete action 'My Action'?"
        );
        assert_eq!(
            get_fmt("Str_UpdateBalloonMsg", &["0.4.8"]),
            "Version 0.4.8 has been downloaded. Restart Orbital to apply."
        );
    }

    #[test]
    fn get_fmt_on_a_missing_key_still_substitutes() {
        let _guard = LANGUAGE_LOCK.lock().expect("language lock");
        set_language("en");
        assert_eq!(get_fmt("Str_Missing_{0}", &["X"]), "Str_Missing_X");
    }

    #[test]
    fn set_language_rejects_unsupported_codes_with_english() {
        let _guard = LANGUAGE_LOCK.lock().expect("language lock");
        set_language("ko");
        assert_eq!(current_language(), "ko");

        set_language("xx");
        assert_eq!(current_language(), "en");
        assert_eq!(get("Str_Save"), "Save");

        set_language("");
        assert_eq!(current_language(), "en");
    }

    #[test]
    fn catalogs_parse_for_every_supported_language() {
        for (index, lang) in supported_languages().iter().enumerate() {
            let parsed = catalog_at(index);
            assert!(
                !parsed.is_empty(),
                "catalog for '{lang}' parsed empty (broken JSON?)"
            );
            match parsed.get("Str_AppTitle") {
                Some(serde_json::Value::String(value)) => {
                    assert!(!value.is_empty(), "empty Str_AppTitle in '{lang}'")
                }
                _ => panic!("Str_AppTitle missing from the '{lang}' catalog"),
            }
        }
    }
}
