//! Native input corpus of AC-007-02 (`fixtures/corpus.json`).
//!
//! `source` separates what the standalone WebKitGTK probes observed (`observed-standalone`, with
//! a document reference) from what is only proposed. Neither is a proof in the product window:
//! the flow must measure, at the PTY, each commit's bytes exactly once and zero preedit bytes.

use std::collections::BTreeSet;

use serde::Deserialize;

pub const CATEGORIES: [&str; 11] = [
    "ime",
    "accents",
    "emoji",
    "cjk",
    "selection",
    "paste",
    "alt-screen",
    "mouse-or-links",
    "scrollback",
    "resize-dpi",
    "a11y",
];

#[derive(Debug, Clone, Deserialize)]
pub struct Corpus {
    pub version: u32,
    pub cjk_standalone_final_hex: String,
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Item {
    pub id: String,
    pub category: String,
    pub driver: String,
    pub ime: Option<String>,
    pub keys: Vec<String>,
    #[serde(default)]
    pub confirm: Vec<String>,
    pub commits: Vec<Commit>,
    pub preedit_pty_bytes: Option<u64>,
    pub source: String,
    #[serde(default)]
    pub evidence: Option<String>,
    #[serde(default)]
    pub expectation: Option<String>,
    #[serde(default)]
    pub pending_on: Option<String>,
    #[serde(default)]
    pub clipboard: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Commit {
    pub text: String,
    pub pty_hex: String,
}

/// Items typed by the parent in the `native-keys` phase (plain keysyms, no input method).
pub const NATIVE_KEY_ITEMS: [&str; 5] = [
    "accents-keysym",
    "deadkey-gtk-compose",
    "return-is-cr",
    "emoji-keysym",
    "cjk-keysym",
];
/// Pinyin items typed by the parent in the `native-ime` phase (private fcitx5).
pub const IME_ITEMS: [&str; 4] = [
    "ime-pinyin-nihao",
    "ime-pinyin-candidate-1",
    "ime-pinyin-candidate-2",
    "ime-pinyin-zhongwen",
];

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn parse(raw: &str) -> Result<Corpus, String> {
    serde_json::from_str(raw).map_err(|e| format!("corpus: {e}"))
}

/// Inconsistencies of the fixture (empty = usable).
pub fn problems(corpus: &Corpus) -> Vec<String> {
    let mut problems = Vec::new();
    if corpus.version != 1 {
        problems.push(format!("version {}", corpus.version));
    }
    let mut ids = BTreeSet::new();
    let mut categories = BTreeSet::new();
    for item in &corpus.items {
        let id = &item.id;
        if !ids.insert(id.as_str()) {
            problems.push(format!("{id}: duplicate id"));
        }
        let category = match item.category.as_str() {
            "mouse" | "links" => "mouse-or-links",
            other => other,
        };
        if !CATEGORIES.contains(&category) {
            problems.push(format!("{id}: unknown category {}", item.category));
        }
        categories.insert(category);
        for commit in &item.commits {
            if hex(commit.text.as_bytes()) != commit.pty_hex {
                problems.push(format!("{id}: pty_hex does not encode {:?}", commit.text));
            }
        }
        match item.source.as_str() {
            "observed-standalone" => {
                if !item
                    .evidence
                    .as_deref()
                    .is_some_and(|e| e.starts_with("docs/PREFLIGHT-"))
                {
                    problems.push(format!("{id}: observed without a preflight reference"));
                }
                if item.pending_on.is_some() {
                    problems.push(format!("{id}: observed item cannot be pending"));
                }
            }
            "proposed" => {}
            other => problems.push(format!("{id}: unknown source {other}")),
        }
        if item.ime.is_some() {
            if item.preedit_pty_bytes != Some(0) {
                problems.push(format!("{id}: IME item must expect zero preedit bytes"));
            }
            if item.confirm.is_empty() {
                problems.push(format!("{id}: IME item without a confirm key"));
            }
        }
        if item.commits.is_empty() && item.expectation.is_none() {
            problems.push(format!("{id}: neither commits nor expectation"));
        }
        if item.driver == "wtype" && item.keys.is_empty() {
            problems.push(format!("{id}: wtype item without keys"));
        }
    }
    for category in CATEGORIES {
        if !categories.contains(category) {
            problems.push(format!("category {category} has no item"));
        }
    }
    problems
}

/// The four Pinyin commits joined as the standalone probe's textarea showed them.
pub fn pinyin_standalone_hex(corpus: &Corpus) -> String {
    let texts: Vec<&str> = corpus
        .items
        .iter()
        .filter(|i| i.ime.as_deref() == Some("fcitx5-pinyin"))
        .flat_map(|i| i.commits.iter().map(|c| c.text.as_str()))
        .collect();
    hex(texts.join("\n").as_bytes())
}

/// Additive native IME contract (root review r1, ESCALAR 14). A clean private fcitx5 profile ranked
/// the same Pinyin candidates in two legitimate orders (run2/run4 vs run5), so the native phase
/// accepts exactly these literal commit sequences for [`IME_ITEMS`] typed with the planned keys
/// (Space, Space, `2`, `1`). The historical `observed-standalone` items stay unchanged.
pub const NATIVE_IME_VARIANTS: [[&str; 4]; 2] = [
    ["你好", "品饮", "拼音", "中文"],
    ["你好", "拼音", "品饮", "中文"],
];

fn window(value: &serde_json::Value) -> Option<(f64, f64)> {
    let a = value.get(0)?.as_f64()?;
    let b = value.get(1)?.as_f64()?;
    (a <= b).then_some((a, b))
}

/// Matches the native IME measurements to one allowed variant (its index).
///
/// `items`: parent measurements in [`IME_ITEMS`] order, each with `preedit_window_ms`,
/// `commit_window_ms` (epoch ms) and `commit_hex` (bytes read at the PTY). `commit_events`: every
/// `compositionend` seen by the terminal target (`data`, `trusted`, `t` epoch ms). Each item must
/// have exactly one trusted non-empty commit event inside its commit window, none in its preedit
/// window, its PTY bytes must be that event's text once, and the texts must be a literal variant.
/// The expectation never comes from the measured bytes.
pub fn native_ime_variant(
    items: &[serde_json::Value],
    commit_events: &[serde_json::Value],
) -> Result<usize, String> {
    let ids: Vec<&str> = items.iter().filter_map(|i| i["id"].as_str()).collect();
    if ids != IME_ITEMS {
        return Err(format!("items {ids:?} are not {IME_ITEMS:?} in order"));
    }
    let commits: Vec<(&str, bool, f64)> = commit_events
        .iter()
        .filter_map(|e| {
            let data = e["data"].as_str()?;
            (!data.is_empty()).then_some((data, e["trusted"] == true, e["t"].as_f64()?))
        })
        .collect();
    if commit_events
        .iter()
        .any(|e| e["data"].as_str().is_some_and(|d| !d.is_empty()) && e["t"].as_f64().is_none())
    {
        return Err("commit event without timestamp".into());
    }
    if commits.iter().any(|(_, trusted, _)| !trusted) {
        return Err("untrusted commit event".into());
    }
    if commits.len() != IME_ITEMS.len() {
        return Err(format!(
            "{} commit events, expected {}",
            commits.len(),
            IME_ITEMS.len()
        ));
    }
    let mut words = Vec::new();
    for item in items {
        let id = item["id"].as_str().unwrap_or("");
        let (pa, pb) = window(&item["preedit_window_ms"]).ok_or(format!("{id}: preedit window"))?;
        let (ca, cb) = window(&item["commit_window_ms"]).ok_or(format!("{id}: commit window"))?;
        if commits.iter().any(|(_, _, t)| (pa..=pb).contains(t)) {
            return Err(format!("{id}: commit event during preedit"));
        }
        let inside: Vec<&str> = commits
            .iter()
            .filter(|(_, _, t)| (ca..=cb).contains(t))
            .map(|(d, _, _)| *d)
            .collect();
        let [word] = inside[..] else {
            return Err(format!(
                "{id}: {} commit events in its window",
                inside.len()
            ));
        };
        if item["commit_hex"].as_str() != Some(hex(word.as_bytes()).as_str()) {
            return Err(format!(
                "{id}: PTY bytes {} are not the committed {word:?} once",
                item["commit_hex"]
            ));
        }
        words.push(word);
    }
    NATIVE_IME_VARIANTS
        .iter()
        .position(|variant| variant[..] == words[..])
        .ok_or_else(|| format!("commits {words:?} match no allowed native variant"))
}
