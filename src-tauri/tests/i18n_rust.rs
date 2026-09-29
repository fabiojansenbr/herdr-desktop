//! Spec 071 (PRD i18n, P3) — the host speaks English. The desktop's Rust side composes every
//! message it hands to the WebView (`RuntimeError.message`, the connection phase label, the
//! attention reason and its guidance) in English; the WebView translates by `code` and by phase.
//!
//! Would catch: a message literal written in Portuguese again anywhere under `src-tauri/src` or
//! `crates/herdr-client/src`, a `LinkPhase::label` left in Portuguese (the sidebar, the home
//! panel and the remote files header all render it), or the folder picker going back to a title
//! fixed in the host instead of the translated one the front sends.
//!
//! AC-071-01: no Portuguese in the host's message literals; `LinkPhase::label` is English.
//! AC-071-03: `project_pick_folder` takes the title from the front and validates it
//!            (non-empty, at most 80 characters, otherwise `Open project`).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use herdr_client::RuntimeError;
use herdr_desktop::connections::failure::AttentionReason;
use herdr_desktop::connections::state::LinkPhase;
use herdr_desktop::project_store::{FolderPicker, ProjectsState, MAX_PICKER_TITLE_CHARS};

// ---------------------------------------------------------------------------------------
// AC-071-01 — the host's messages are in English
// ---------------------------------------------------------------------------------------

/// Letters no English message of this product uses.
const MARKS: [char; 12] = ['ã', 'õ', 'ç', 'á', 'é', 'í', 'ó', 'ú', 'â', 'ê', 'ô', 'à'];

/// Portuguese function words: the giveaway of a Portuguese sentence, whatever its vocabulary —
/// which is what a list of content words alone keeps missing ("thread da ponte", "pixels de mouse
/// exigem geometria"). Deliberately without the ones English shares as whole words (`a`, `as`,
/// `no`, `o`, `e`, `se`, `por`, `ou`, `um`, `com`, `mas`), because these messages are English.
/// Spanish shares many of them, which does not matter: only the host's literals are scanned here,
/// and the host has one language.
const FUNCTION_WORDS: &[&str] = &[
    "de", "do", "da", "dos", "das", "os", "uma", "em", "na", "nas", "nos", "que", "ao", "aos",
    "sem", "para", "nao", "foi", "eram", "sera", "mais", "ainda", "ja", "pelo", "pela", "pelos",
    "pelas", "este", "esta", "esse", "essa", "aquele", "aquela", "deste", "desta", "neste",
    "nesta", "seu", "sua", "seus", "suas", "como", "tambem", "so", "nada", "ela", "ele", "eles",
    "elas", "isso", "dois", "duas", "muito", "pouco", "antes", "depois", "agora", "sempre",
    "nunca", "cada", "outro", "outra", "outros", "outras", "todos", "todas", "tudo", "nenhum",
    "nenhuma", "algum", "alguma", "quando", "onde", "porque", "entao", "apos", "entre", "desde",
    "ate",
];

/// Content words of the messages this spec migrated, for the sentences short enough to carry no
/// function word at all ("frame inválido", "stdio ausente"). Accent-free on purpose: `MARKS`
/// already catches the accented spellings. None of these is an English word — `thread`, `pixels`
/// and `diverge` are deliberately absent for that reason.
const CONTENT_WORDS: &[&str] = &[
    "falha",
    "falhou",
    "desconectado",
    "encerrou",
    "encerrado",
    "encerrada",
    "desconhecida",
    "desconhecido",
    "tecla",
    "ausente",
    "evento",
    "eventos",
    "resposta",
    "inesperada",
    "serializar",
    "executar",
    "projetos",
    "servidor",
    "arquivo",
    "arquivos",
    "pasta",
    "raiz",
    "perfil",
    "caminho",
    "enviado",
    "enviar",
    "alterado",
    "alvo",
    "precisa",
    "abrir",
    "abra",
    "gravar",
    "mudou",
    "existe",
    "vazio",
    "vazia",
    "linhas",
    "apenas",
    "somente",
    "aguarda",
    "aguardando",
    "escreva",
    "informe",
    "verifique",
    "selecione",
    "recarregue",
    "atualize",
    "instale",
    "inicie",
    "iniciar",
    "habilite",
    "carregue",
    "confirme",
    "painel",
    "conteudo",
    "cancelado",
    "cancelada",
    "identidade",
    "estabelecida",
    "botao",
    "geometria",
    "exigem",
    "desatualizada",
    "desatualizado",
    "ponte",
    "leitura",
    "porta",
    "computador",
    "voce",
    // Adjectives whose Portuguese spelling differs from the English one, so position is not needed.
    "remoto",
    "remota",
    "remotos",
    "remotas",
    "nativo",
    "nativa",
    "interno",
    "interna",
    "externo",
    "externa",
    "atual",
    "atuais",
];

/// Literals that are not product text and stay as they are:
///
/// * developer traces of `HERDR_DESKTOP_TRACE` (step names and outcomes) and mutex/thread names,
///   which never reach the WebView and are read by whoever is debugging a connection;
/// * the startup line printed to stderr before any window exists.
///
/// Kept honest by `ac_071_01_the_allowlist_holds_only_what_is_still_there`.
const NOT_PRODUCT_TEXT: &[&str] = &[
    // traces and lock/thread names
    "ponte",
    "servidor",
    "superfície",
    "autenticação",
    "lock do hub",
    "aguardando geometria",
    "stdio ausente",
    "frame inválido",
    "sem conector",
    "sem suporte (remote-api-bridge)",
    "cancelado pelo diálogo",
    "registrada (host sem conexão)",
    "sem envio (interesse já vigente)",
    "renegociando (sem surface_interest)",
    "timeout de {budget} ao iniciar",
    "falha ao enviar hello",
    "falha ao iniciar reader",
    "falha ao iniciar thread de abertura",
    // stderr, outside any window: the startup line and the panic line next to it
    "herdr-desktop: falha ao iniciar",
    "herdr-desktop: erro interno (unexpected_panic)",
];

/// Adjectives spelled the same in Portuguese and English. No word list can tell those two
/// languages apart — only the position can: English puts the adjective before the noun
/// ("the local socket"), Portuguese after it ("socket local").
const AMBIGUOUS_ADJECTIVES: &[&str] = &[
    "local",
    "global",
    "normal",
    "central",
    "principal",
    "virtual",
    "anterior",
    "posterior",
    "final",
    "total",
    "original",
    "similar",
    "regular",
    "interior",
    "exterior",
    "superior",
    "inferior",
];

/// English words that legitimately come right before an adjective. Anything else before one of
/// [`AMBIGUOUS_ADJECTIVES`] is a noun, and a noun before its adjective is Portuguese order. The
/// list fails closed on purpose: a new English message that trips it says which word to add.
const BEFORE_ADJECTIVE: &[&str] = &[
    "the", "a", "an", "this", "that", "these", "those", "its", "their", "our", "your", "my", "no",
    "not", "another", "other", "one", "and", "or", "but", "is", "are", "was", "were", "be", "been",
    "being", "as", "of", "to", "in", "on", "for", "from", "with", "without", "by", "into", "than",
    "more", "most", "very", "too", "still", "only", "already", "any", "every", "each", "some",
    "both", "either", "neither", "non", "use", "uses", "open", "opens", "read", "reads", "write",
    "writes", "start", "starts", "keep", "keeps", "made", "make", "makes", "became", "becomes",
];

/// Words of a sentence. `_` stays inside a word, so the code `endpoint_local` is one identifier
/// and never reads as the two words "endpoint local".
fn sentence_words(lower: &str) -> Vec<&str> {
    lower
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|word| !word.is_empty())
        .collect()
}

/// A same-spelling adjective sitting after its noun: Portuguese word order in what should be an
/// English sentence. Only literals with a space are considered — the rule is about word order, and
/// an identifier or a path (`herdr-desktop-local`) is not a sentence.
fn portuguese_word_order(lower: &str) -> Option<String> {
    if !lower.contains(' ') {
        return None;
    }
    let words = sentence_words(lower);
    words.windows(2).find_map(|pair| {
        let (before, word) = (pair[0], pair[1]);
        (AMBIGUOUS_ADJECTIVES.contains(&word) && !BEFORE_ADJECTIVE.contains(&before))
            .then(|| format!("{before} {word}"))
    })
}

fn looks_portuguese(text: &str) -> bool {
    let lower = text.to_lowercase();
    if lower.chars().any(|c| MARKS.contains(&c)) {
        return true;
    }
    if portuguese_word_order(&lower).is_some() {
        return true;
    }
    lower
        .split(|c: char| !c.is_alphanumeric())
        .any(|word| FUNCTION_WORDS.contains(&word) || CONTENT_WORDS.contains(&word))
}

/// Functions that compose a `RuntimeError` message (directly or through the private helpers of
/// `files/**` and `herdr-client`). Every string literal inside one of their call argument lists is
/// product text, so no allowlist applies to it.
const MESSAGE_BUILDERS: &[&str] = &[
    "RuntimeError::new",
    "RuntimeError::from_io_kind",
    "ConnectFailure::transient",
    "ConnectFailure::attention_message",
    "file_error",
    "framing_error",
    "io_error",
    "invalid",
    "error",
    "retryable",
];

/// The balanced argument list of every call to `name`, as raw source text. Quotes are tracked so a
/// parenthesis inside a string never closes the list, and a call whose name is the tail of a
/// longer identifier is skipped (`io_error` is not a call to `error`).
fn call_arguments<'a>(source: &'a str, name: &str) -> Vec<&'a str> {
    let bytes = source.as_bytes();
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(offset) = source[from..].find(name) {
        let start = from + offset;
        from = start + 1;
        if start > 0 && (bytes[start - 1].is_ascii_alphanumeric() || bytes[start - 1] == b'_') {
            continue;
        }
        let mut open = start + name.len();
        while open < bytes.len() && (bytes[open] as char).is_whitespace() {
            open += 1;
        }
        if open >= bytes.len() || bytes[open] != b'(' {
            continue;
        }
        let mut depth = 0usize;
        let mut in_string = false;
        let mut index = open;
        while index < bytes.len() {
            match bytes[index] {
                b'\\' if in_string => index += 1,
                b'"' => in_string = !in_string,
                b'(' if !in_string => depth += 1,
                b')' if !in_string => {
                    depth -= 1;
                    if depth == 0 {
                        found.push(&source[open + 1..index]);
                        break;
                    }
                }
                _ => {}
            }
            index += 1;
        }
    }
    found
}

/// Every string literal of `source`, with `//` comment lines skipped (the module docs and the
/// decision comments of this codebase are written in Portuguese on purpose).
fn string_literals(source: &str) -> Vec<String> {
    let mut found = Vec::new();
    for line in source.lines() {
        if line.trim_start().starts_with("//") {
            continue;
        }
        let bytes: Vec<char> = line.chars().collect();
        let mut index = 0;
        while index < bytes.len() {
            if bytes[index] != '"' {
                index += 1;
                continue;
            }
            let mut literal = String::new();
            index += 1;
            while index < bytes.len() && bytes[index] != '"' {
                if bytes[index] == '\\' && index + 1 < bytes.len() {
                    literal.push(bytes[index]);
                    index += 1;
                }
                literal.push(bytes[index]);
                index += 1;
            }
            index += 1;
            found.push(literal);
        }
    }
    found
}

fn rust_files(root: &Path, into: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, into);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            into.push(path);
        }
    }
}

/// Every `.rs` file of the desktop host, with its text: `src-tauri/src/**` and the client crate.
fn host_sources() -> Vec<(PathBuf, String)> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    rust_files(&manifest.join("src"), &mut files);
    rust_files(
        &manifest
            .join("../crates/herdr-client/src")
            .canonicalize()
            .unwrap(),
        &mut files,
    );
    assert!(files.len() > 20, "source trees not found: {files:?}");
    files.sort();
    files
        .into_iter()
        .map(|path| {
            let text = std::fs::read_to_string(&path).unwrap();
            (path, text)
        })
        .collect()
}

/// The tightest guard: the literals the host hands to a `RuntimeError` builder. No allowlist —
/// every one of them is text a user can read.
#[test]
fn ac_071_01_every_runtime_error_message_is_english() {
    let mut offenders: Vec<String> = Vec::new();
    let mut scanned = 0usize;
    for (file, source) in host_sources() {
        for builder in MESSAGE_BUILDERS {
            for arguments in call_arguments(&source, builder) {
                for literal in string_literals(arguments) {
                    scanned += 1;
                    if looks_portuguese(&literal) {
                        offenders.push(format!("{}: {builder}(… {literal:?})", file.display()));
                    }
                }
            }
        }
    }
    assert!(
        scanned > 500,
        "the builders were not found in the sources ({scanned} literals)"
    );
    offenders.sort();
    offenders.dedup();
    assert!(
        offenders.is_empty(),
        "{} RuntimeError message(s) still in Portuguese:\n{}",
        offenders.len(),
        offenders.join("\n")
    );
}

/// The detector itself, on the class of failure that slipped past two rounds of this spec: a
/// message whose every word is English but whose word order is Portuguese ("socket local"). A word
/// list cannot see it, so this pins the rule that can.
///
/// Would catch: `AMBIGUOUS_ADJECTIVES` or the word-order rule being dropped or weakened, which
/// would let `"socket local"` back in even with the literal itself still translated.
#[test]
fn ac_071_01_the_detector_rejects_portuguese_word_order() {
    for portuguese in [
        // The literal this round translated, and its siblings.
        "socket local",
        "canal local",
        "cache local",
        "buffer local",
        "endpoint principal",
        "surface virtual",
        // Adjectives Portuguese spells differently: the word list is enough for these.
        "host remoto",
        "provider nativo",
        "estado interno",
        "geometria atual",
    ] {
        assert!(
            looks_portuguese(portuguese),
            "{portuguese:?} must be rejected as Portuguese"
        );
    }
    for english in [
        // The same words in English order.
        "local socket",
        "the local host",
        "this local provider",
        "a local session",
        "no fallback to the local host",
        "this local provider does not open remote host files",
        "the final frame is not the original one",
        // Not sentences: a code and a thread name keep their shape.
        "local",
        "endpoint_local",
        "herdr-desktop-local",
    ] {
        assert!(
            !looks_portuguese(english),
            "{english:?} is English and must pass"
        );
    }
}

/// Would catch: an entry kept after the literal it excused was translated or deleted, which would
/// silently widen the guard above.
#[test]
fn ac_071_01_the_allowlist_holds_only_what_is_still_there() {
    let mut literals: Vec<String> = Vec::new();
    for (_, source) in host_sources() {
        literals.extend(string_literals(&source));
    }
    for excused in NOT_PRODUCT_TEXT {
        assert!(
            looks_portuguese(excused),
            "{excused:?} is not detected as Portuguese, so it needs no entry"
        );
        assert!(
            literals.iter().any(|literal| literal == excused),
            "{excused:?} is no longer in the sources; drop the entry"
        );
    }
}

#[test]
fn ac_071_01_no_portuguese_message_literal_in_the_host() {
    let mut offenders: Vec<String> = Vec::new();
    for (file, source) in host_sources() {
        for literal in string_literals(&source) {
            if NOT_PRODUCT_TEXT.contains(&literal.as_str()) {
                continue;
            }
            if looks_portuguese(&literal) {
                offenders.push(format!("{}: {literal:?}", file.display()));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "{} host literal(s) still in Portuguese:\n{}",
        offenders.len(),
        offenders.join("\n")
    );
}

#[test]
fn ac_071_01_link_phase_labels_are_english() {
    assert_eq!(
        [
            LinkPhase::Offline.label(),
            LinkPhase::Connecting.label(),
            LinkPhase::Online.label(),
            LinkPhase::Reconnecting.label(),
            LinkPhase::Attention.label(),
        ],
        [
            "Offline",
            "Connecting",
            "Online",
            "Reconnecting",
            "Needs attention",
        ]
    );
}

#[test]
fn ac_071_01_attention_reasons_keep_their_codes_and_speak_english() {
    let reasons = [
        (AttentionReason::HostKeyUnknown, "ssh_host_key_unknown"),
        (AttentionReason::HostKeyChanged, "ssh_host_key_changed"),
        (
            AttentionReason::AuthenticationRequired,
            "ssh_authentication_required",
        ),
        (AttentionReason::SshUnavailable, "ssh_unavailable"),
        (AttentionReason::HerdrMissing, "remote_herdr_missing"),
        (AttentionReason::HerdrOutdated, "remote_herdr_outdated"),
        (
            AttentionReason::ServerNotRunning,
            "remote_server_not_running",
        ),
        (
            AttentionReason::ServerIncompatible,
            "remote_server_incompatible",
        ),
    ];
    for (reason, code) in reasons {
        assert_eq!(reason.code(), code, "code of {reason:?} must not change");
        assert!(
            !looks_portuguese(reason.message()),
            "message of {reason:?} is Portuguese: {}",
            reason.message()
        );
        assert!(
            !looks_portuguese(reason.guidance()),
            "guidance of {reason:?} is Portuguese: {}",
            reason.guidance()
        );
    }
}

// ---------------------------------------------------------------------------------------
// AC-071-03 — the folder picker title comes from the front, validated
// ---------------------------------------------------------------------------------------

#[derive(Default)]
struct RecordingPicker {
    titles: Mutex<Vec<String>>,
}

impl FolderPicker for RecordingPicker {
    fn pick_folder(&self, title: &str) -> Result<Option<String>, RuntimeError> {
        self.titles.lock().unwrap().push(title.to_owned());
        Ok(Some("/tmp/chosen".into()))
    }
}

fn projects_state(dir: &Path) -> (ProjectsState, Arc<RecordingPicker>) {
    let state = ProjectsState::new(dir.join("prefs"), dir.join("herdr"));
    let picker = Arc::new(RecordingPicker::default());
    state.set_folder_picker(picker.clone());
    (state, picker)
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("herdr-071-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn ac_071_03_picker_uses_the_title_sent_by_the_front() {
    let dir = temp_dir("title");
    let (state, picker) = projects_state(&dir);
    for title in ["Open project", "Abrir projeto", "Abrir proyecto"] {
        let chosen = tauri::async_runtime::block_on(state.pick_folder(title.to_owned())).unwrap();
        assert_eq!(chosen.as_deref(), Some("/tmp/chosen"));
    }
    assert_eq!(
        picker.titles.lock().unwrap().clone(),
        ["Open project", "Abrir projeto", "Abrir proyecto"]
    );
}

#[test]
fn ac_071_03_an_empty_or_oversized_title_falls_back_to_english() {
    let dir = temp_dir("fallback");
    let (state, picker) = projects_state(&dir);
    let long = "x".repeat(MAX_PICKER_TITLE_CHARS + 1);
    for title in ["", "   ", long.as_str()] {
        tauri::async_runtime::block_on(state.pick_folder(title.to_owned())).unwrap();
    }
    assert_eq!(
        picker.titles.lock().unwrap().clone(),
        ["Open project", "Open project", "Open project"]
    );
    // The boundary is accepted as sent.
    let boundary = "y".repeat(MAX_PICKER_TITLE_CHARS);
    tauri::async_runtime::block_on(state.pick_folder(boundary.clone())).unwrap();
    assert_eq!(picker.titles.lock().unwrap().last().unwrap(), &boundary);
}
