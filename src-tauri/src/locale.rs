//! Language of the window (spec 067, PRD i18n P1).
//!
//! The host owns the decision because the WebView has no environment: `HERDR_DESKTOP_LOCALE`
//! wins (the real-window harnesses fix `pt` so selectors written in Portuguese stay stable),
//! then the system locale, then English. The value is the raw tag as the system publishes it
//! (`pt_BR.UTF-8`, `es-MX`, `C`); normalising it to one of the three languages is the front's
//! `resolveLocale`, which is the only place that knows which languages the product carries.

/// Commands this module exposes (kept in sync with `lib.rs` by the registry test).
pub const COMMANDS: &[&str] = &["app_locale"];

/// Override read before the system locale; blank (or absent) is the same as unset.
pub const ENV_LOCALE: &str = "HERDR_DESKTOP_LOCALE";

/// Language used when neither the override nor the system publishes one.
pub const DEFAULT_LOCALE: &str = "en";

/// Resolution over injected providers, so the rule is testable without touching the process
/// environment (nextest runs the tests of one binary in the same process).
pub fn locale_from(
    env: impl Fn(&str) -> Option<String>,
    system: impl Fn() -> Option<String>,
) -> String {
    if let Some(value) = env(ENV_LOCALE) {
        if !value.trim().is_empty() {
            return value;
        }
    }
    match system() {
        Some(tag) if !tag.trim().is_empty() => tag,
        _ => DEFAULT_LOCALE.to_owned(),
    }
}

/// The environment's language tag, for the front's boot. No secret and no other environment
/// value ever crosses this command.
#[tauri::command]
pub fn app_locale() -> String {
    locale_from(|key| std::env::var(key).ok(), sys_locale::get_locale)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_of(value: Option<&str>) -> impl Fn(&str) -> Option<String> + '_ {
        move |key| {
            if key == ENV_LOCALE {
                value.map(str::to_owned)
            } else {
                panic!("no other environment value is read: {key}")
            }
        }
    }

    // AC-067-01 — would catch: the override being ignored (the E2E harnesses would then follow
    // the host's language), or it being normalised here instead of in the front.
    #[test]
    fn the_override_wins_over_the_system_locale() {
        assert_eq!(
            locale_from(env_of(Some("pt")), || Some("en-US".to_owned())),
            "pt"
        );
        assert_eq!(
            locale_from(env_of(Some("pt_BR.UTF-8")), || None),
            "pt_BR.UTF-8",
            "the raw tag is passed through; the front resolves it"
        );
    }

    // AC-067-01 — would catch: an empty override shadowing the system locale, or a system
    // without a locale ending up with an empty language instead of English.
    #[test]
    fn an_empty_override_falls_back_to_the_system_and_then_to_english() {
        assert_eq!(
            locale_from(env_of(Some("   ")), || Some("es-MX".to_owned())),
            "es-MX"
        );
        assert_eq!(
            locale_from(env_of(None), || Some("es-MX".to_owned())),
            "es-MX"
        );
        assert_eq!(locale_from(env_of(None), || None), DEFAULT_LOCALE);
        assert_eq!(locale_from(env_of(Some("")), || Some(" ".to_owned())), "en");
    }

    // Would catch: the command reading the process environment for something else, or the real
    // system provider panicking (it is the product path of `app_locale`).
    #[test]
    fn the_command_returns_a_non_empty_tag() {
        assert!(!app_locale().trim().is_empty());
        assert_eq!(COMMANDS, ["app_locale"]);
    }
}
