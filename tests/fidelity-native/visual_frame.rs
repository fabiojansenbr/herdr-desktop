//! `visual-frame` phase of the native flow (spec 010, AC-010-01/02/03): literal expectations, the
//! parent's pure helpers (viewport command, key tokens, engine expectations) and the evaluator.
//!
//! The page (`src/features/fidelity/visual-frame.ts`) reports raw DOM observations of the composed
//! window at a 1440×900 CSS viewport; the parent resizes the private output, presses native keys,
//! measures PTY bytes and reads the engine. Values the checks compare against come from this file
//! (design guide literals) or from the parent ledger (engine, PTY), never from the page's report.

use serde_json::{json, Value};

pub use super::paste_flow::ParentLedger as Ledger;

pub const PHASE: &str = "visual-frame";
pub const STEPS: [&str; 6] = [
    "visual-viewport",
    "visual-engine",
    "visual-ctrlk-outside",
    "visual-escape",
    "visual-ctrlk-terminal",
    "visual-restore",
];
pub const CHECKS: [&str; 9] = [
    "frame_dimensions_at_1440x900",
    "guide_tokens_on_root",
    "ui_inter_terminal_jetbrains_mono",
    "top_bar_order_menus_and_host",
    "status_bar_real_session",
    "small_text_contrast_at_least_4_5",
    "palette_opens_within_one_frame_with_engine_lists",
    "escape_closes_and_restores_focus",
    "ctrl_k_in_terminal_reaches_pty_once",
];
pub const TARGET: (f64, f64) = (1440.0, 900.0);
pub const OUTPUT: &str = "HEADLESS-1";
/// Output mode the flow's display starts with and returns to (`view_flow::STAGES` restore).
pub const RESTORE_MODE: (u32, u32) = (1280, 720);

/// design/herdr-desktop.pen variables (design/GUIA-IMPLEMENTACAO.md).
pub const GUIDE_TOKENS: [(&str, &str); 16] = [
    ("--bg", "#0B0C10"),
    ("--surface", "#111318"),
    ("--surface-2", "#171A21"),
    ("--surface-3", "#1E222B"),
    ("--border", "#242833"),
    ("--text", "#E7E9EE"),
    ("--text-muted", "#8C93A3"),
    ("--text-dim", "#5B6272"),
    ("--accent", "#8FA8FF"),
    ("--accent-soft", "#8FA8FF1F"),
    ("--working", "#5BD68A"),
    ("--attention", "#F4B454"),
    ("--error", "#F2777A"),
    ("--idle", "#6B7280"),
    ("--font-ui", "Inter"),
    ("--font-mono", "JetBrains Mono"),
];
/// Region → (dimension, px): heights of the bars, widths of the columns and of the center.
pub const REGIONS: [(&str, &str, f64); 6] = [
    ("topbar", "height", 44.0),
    ("activity", "width", 52.0),
    ("projects", "width", 272.0),
    ("agents", "width", 256.0),
    ("status", "height", 26.0),
    ("center", "width", 860.0),
];
pub const TOPBAR_ORDER: [&str; 13] = [
    "logo",
    "menu",
    "menu",
    "menu",
    "menu",
    "menu",
    "search",
    "search-hint",
    "host",
    "notifications",
    "window-minimize",
    "window-maximize",
    "window-close",
];
pub const MENUS: [&str; 5] = ["Arquivo", "Editar", "Ver", "Agentes", "Janela"];
/// Existing window actions a menu item may call (`MENU_ACTIONS` of src/components/frame/menus.ts).
pub const MENU_ACTIONS: [&str; 11] = [
    "showProjects",
    "openConnections",
    "paste",
    "toggleProjects",
    "toggleAgents",
    "toggleFiles",
    "openPalette",
    "newAgent",
    "split",
    "newTab",
    "reconnect",
];
pub const SEARCH_TEXT: &str = "Buscar projetos, panes, agentes e comandos";
pub const SEARCH_HINT: &str = "Ctrl K";
pub const PALETTE_SECTIONS: [&str; 4] = ["Projetos", "Panes", "Agentes", "Comandos"];
pub const TERMINAL_TARGET: &str = "Terminal Herdr";
pub const MIN_SMALL_TEXT_RATIO: f64 = 4.5;
/// Branch of the Local fixture repository (`remote.rs` runs `git init -b` on `local-project`): a
/// value the status bar can only show when App passes the engine's branch (≠ "—", shown without).
pub use super::remote::LOCAL_GIT_BRANCH as FIXTURE_BRANCH;
/// How long the parent waits for the engine to list the agent it started for this phase.
pub const AGENT_WAIT_MS: u64 = 30_000;
const TOLERANCE: f64 = 1.0;

/// `wtype` words of the keys this phase presses.
pub fn wtype_args(token: &str) -> Result<Vec<String>, String> {
    let words: &[&str] = match token {
        "ctrl+k" => &["-M", "ctrl", "-k", "k", "-m", "ctrl"],
        "Escape" => &["-k", "Escape"],
        _ => return Err(format!("key {token:?} is not allowed in {PHASE}")),
    };
    Ok(words.iter().map(|w| (*w).to_owned()).collect())
}

fn num(v: &Value) -> Option<f64> {
    v.as_f64()
}

/// swaymsg words that make the CSS viewport `TARGET`: the current mode of the private output plus
/// the difference between the target and the viewport the page observed (window decorations
/// included). Refuses a scaled output or a viewport larger than its output.
pub fn viewport_command(
    outputs: &Value,
    inner_width: f64,
    inner_height: f64,
) -> Result<Vec<String>, String> {
    let output = outputs
        .as_array()
        .and_then(|all| all.iter().find(|o| o["name"] == OUTPUT))
        .ok_or_else(|| format!("{OUTPUT} not in get_outputs"))?;
    if num(&output["scale"]) != Some(1.0) {
        return Err(format!("{OUTPUT} scale {} is not 1", output["scale"]));
    }
    let (Some(width), Some(height)) = (
        num(&output["current_mode"]["width"]),
        num(&output["current_mode"]["height"]),
    ) else {
        return Err(format!("{OUTPUT} has no current mode"));
    };
    if inner_width <= 0.0 || inner_height <= 0.0 || inner_width > width || inner_height > height {
        return Err(format!(
            "viewport {inner_width}x{inner_height} does not fit output {width}x{height}"
        ));
    }
    let target_w = width + TARGET.0 - inner_width;
    let target_h = height + TARGET.1 - inner_height;
    Ok(mode_command(
        target_w.round() as u32,
        target_h.round() as u32,
    ))
}

pub fn mode_command(width: u32, height: u32) -> Vec<String> {
    [
        "output",
        OUTPUT,
        "resolution",
        &format!("{width}x{height}"),
        "scale",
        "1",
    ]
    .map(str::to_owned)
    .to_vec()
}

/// Engine literals of the session the page shows: version (from `herdr --version`), channel, tabs
/// of the session, panes of the confirmed pane's tab and the branch expected in the status bar.
pub fn engine_expectations(
    version_out: &str,
    tab_list: &Value,
    pane_list: &Value,
    pane_id: &str,
    git_branch: Option<&str>,
) -> Result<Value, String> {
    let version = version_out
        .trim()
        .strip_prefix("herdr ")
        .filter(|v| !v.is_empty() && !v.contains(char::is_whitespace))
        .ok_or_else(|| format!("unexpected herdr --version output {version_out:?}"))?;
    let tabs = tab_list["result"]["tabs"]
        .as_array()
        .ok_or("tab list without result.tabs")?;
    let panes = pane_list["result"]["panes"]
        .as_array()
        .ok_or("pane list without result.panes")?;
    let tab = panes
        .iter()
        .find(|p| p["pane_id"] == pane_id)
        .and_then(|p| p["tab_id"].as_str())
        .ok_or_else(|| format!("pane {pane_id} not in the engine pane list"))?;
    let mut in_tab: Vec<&str> = panes
        .iter()
        .filter(|p| p["tab_id"] == tab)
        .filter_map(|p| p["pane_id"].as_str())
        .collect();
    in_tab.sort_unstable();
    let all: Vec<&str> = panes.iter().filter_map(|p| p["pane_id"].as_str()).collect();
    Ok(json!({
        "version": version,
        "channel": if version.contains("-preview") { "preview" } else { "stable" },
        "tabs": tabs.len(),
        "tab_id": tab,
        "pane_ids_in_tab": in_tab,
        "pane_ids": all,
        "branch": git_branch.filter(|b| !b.is_empty() && *b != "HEAD").unwrap_or("—"),
    }))
}

/// `agent:<pane_id>` of every agent in the engine's `herdr agent list` answer, sorted (the palette's
/// Agentes ids).
pub fn engine_agent_ids(agent_list: &Value) -> Result<Vec<String>, String> {
    let agents = agent_list["result"]["agents"]
        .as_array()
        .ok_or_else(|| format!("agent list without result.agents: {agent_list}"))?;
    let mut ids = agents
        .iter()
        .map(|a| {
            a["pane_id"]
                .as_str()
                .map(|p| format!("agent:{p}"))
                .ok_or_else(|| format!("agent without pane_id: {a}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    ids.sort_unstable();
    Ok(ids)
}

/// `project:<id>` of every project in the window's catalog (`projects.json`), sorted (the
/// palette's Projetos ids).
pub fn catalog_project_ids(catalog: &Value) -> Result<Vec<String>, String> {
    let projects = catalog["projects"]
        .as_array()
        .ok_or_else(|| format!("project catalog without projects: {catalog}"))?;
    let mut ids = projects
        .iter()
        .map(|p| {
            p["id"]
                .as_str()
                .map(|id| format!("project:{id}"))
                .ok_or_else(|| format!("project without id: {p}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    ids.sort_unstable();
    Ok(ids)
}

/// Pane of the session where the parent starts the fixture agent: any pane other than the
/// confirmed one, whose PTY this phase measures.
pub fn agent_host_pane(pane_list: &Value, confirmed: &str) -> Result<String, String> {
    pane_list["result"]["panes"]
        .as_array()
        .and_then(|panes| {
            panes
                .iter()
                .filter_map(|p| p["pane_id"].as_str())
                .find(|p| *p != confirmed)
        })
        .map(str::to_owned)
        .ok_or_else(|| format!("no pane other than {confirmed} to start the agent in"))
}

// ------------------------------------------------------------------------------ evaluator

#[derive(Debug, Clone, Copy)]
struct Rgba {
    r: f64,
    g: f64,
    b: f64,
    a: f64,
}

/// `rgb()`/`rgba()` (comma or space syntax) and `#rrggbb[aa]` as computed styles serialize them.
fn parse_color(css: &str) -> Option<Rgba> {
    let text = css.trim().to_ascii_lowercase();
    if let Some(hex) = text.strip_prefix('#') {
        let byte = |i: usize| {
            u8::from_str_radix(hex.get(i..i + 2)?, 16)
                .ok()
                .map(f64::from)
        };
        return match hex.len() {
            6 | 8 => Some(Rgba {
                r: byte(0)?,
                g: byte(2)?,
                b: byte(4)?,
                a: if hex.len() == 8 {
                    byte(6)? / 255.0
                } else {
                    1.0
                },
            }),
            _ => None,
        };
    }
    let inner = text
        .strip_prefix("rgba(")
        .or_else(|| text.strip_prefix("rgb("))?
        .strip_suffix(')')?;
    let parts: Vec<f64> = inner
        .split(|c: char| c == ',' || c == '/' || c.is_whitespace())
        .filter(|p| !p.is_empty())
        .map(|p| match p.strip_suffix('%') {
            Some(pct) => pct.parse::<f64>().map(|v| v / 100.0),
            None => p.parse::<f64>(),
        })
        .collect::<Result<_, _>>()
        .ok()?;
    match parts[..] {
        [r, g, b] => Some(Rgba { r, g, b, a: 1.0 }),
        [r, g, b, a] => Some(Rgba { r, g, b, a }),
        _ => None,
    }
}

fn luminance(c: Rgba) -> f64 {
    let lin = |v: f64| {
        let s = v / 255.0;
        if s <= 0.04045 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(c.r) + 0.7152 * lin(c.g) + 0.0722 * lin(c.b)
}

/// WCAG ratio of `color` (alpha × `opacity`) over the opaque `background`.
pub fn contrast(color: &str, background: &str, opacity: f64) -> Option<f64> {
    let fg = parse_color(color)?;
    let bg = parse_color(background)?;
    let a = (fg.a * opacity).clamp(0.0, 1.0);
    let mix = |f: f64, b: f64| f * a + b * (1.0 - a);
    let over = Rgba {
        r: mix(fg.r, bg.r),
        g: mix(fg.g, bg.g),
        b: mix(fg.b, bg.b),
        a: 1.0,
    };
    let (l1, l2) = (luminance(over), luminance(bg));
    Some((l1.max(l2) + 0.05) / (l1.min(l2) + 0.05))
}

fn first_family(list: &str) -> String {
    list.split(',')
        .next()
        .unwrap_or("")
        .trim()
        .trim_matches(|c| c == '"' || c == '\'')
        .to_owned()
}

fn field<'a>(v: &'a Value, key: &str, what: &str) -> Result<&'a Value, String> {
    match v.get(key) {
        Some(found) if !found.is_null() => Ok(found),
        _ => Err(format!("{PHASE}: {what} has no {key}")),
    }
}

fn text(v: &Value) -> &str {
    v.as_str().unwrap_or("")
}

fn close(v: &Value, want: f64) -> bool {
    num(v).is_some_and(|got| (got - want).abs() <= TOLERANCE)
}

/// One trusted Ctrl+K keydown on `target` with the palette present (or absent) at the next frame.
fn one_trusted_key(keys: &Value, target: &str, palette: bool) -> bool {
    keys.as_array().is_some_and(|k| {
        k.len() == 1
            && k[0]["trusted"] == true
            && k[0]["target"] == target
            && k[0]["frames_waited"] == 1
            && k[0]["palette_at_first_frame"] == palette
    })
}

pub fn checks(report: &Value, ledger: &Ledger) -> Result<Vec<(&'static str, bool)>, String> {
    if report["phase"] != PHASE {
        return Err(format!("{PHASE}: report of phase {}", report["phase"]));
    }
    if !report["error"].is_null() {
        return Err(format!("{PHASE}: page error {}", report["error"]));
    }
    let answer = |step: &str| {
        ledger
            .steps
            .get(step)
            .ok_or_else(|| format!("{PHASE}: parent never answered {step}"))
    };
    for step in STEPS {
        answer(step)?;
    }
    let engine = field(answer("visual-engine")?, "engine", "visual-engine")?;
    let identity = field(report, "identity", "report")?;
    let frame = field(report, "frame", "report")?;
    let viewport = field(report, "viewport", "report")?;

    // AC-010-01 — dimensions.
    let regions = field(frame, "regions", "frame")?;
    let dims_ok = close(&viewport["inner_width"], TARGET.0)
        && close(&viewport["inner_height"], TARGET.1)
        && viewport["dpr"] == 1
        && close(&regions["topbar"]["width"], TARGET.0)
        && REGIONS
            .iter()
            .all(|(region, dim, px)| close(&regions[*region][*dim], *px));

    // AC-010-01 — tokens and fonts.
    let tokens = field(frame, "tokens", "frame")?;
    let tokens_ok = GUIDE_TOKENS.iter().all(|(name, value)| {
        let got = text(&tokens[*name]);
        if name.starts_with("--font") {
            first_family(got) == *value
        } else {
            got.eq_ignore_ascii_case(value)
        }
    });
    let fonts = field(frame, "fonts", "frame")?;
    let fonts_ok = first_family(text(&fonts["topbar"])) == "Inter"
        && first_family(text(&fonts["status"])) == "Inter"
        && first_family(text(&fonts["terminal"])) == "JetBrains Mono"
        && text(&fonts["canvas"]).contains("JetBrains Mono");

    // AC-010-02 — top bar, menus, host.
    let items = frame["topbar_items"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let kinds: Vec<&str> = items.iter().map(|i| text(&i["item"])).collect();
    let item = |kind: &str| items.iter().find(|i| i["item"] == kind);
    let menu_texts: Vec<&str> = items
        .iter()
        .filter(|i| i["item"] == "menu")
        .map(|i| text(&i["text"]))
        .collect();
    let endpoint = text(&identity["endpoint"]);
    let host = &frame["host"];
    let notifications_ok = item("notifications")
        .is_some_and(|i| i["disabled"] == true && !text(&i["label"]).is_empty());
    let window_ok = ["window-minimize", "window-maximize", "window-close"]
        .iter()
        .all(|k| item(k).is_some_and(|i| i["disabled"] == false && !text(&i["label"]).is_empty()));
    let icons_ok = notifications_ok && window_ok;
    let menus = report["menus"].as_array().cloned().unwrap_or_default();
    let mut enabled_actions: Vec<String> = Vec::new();
    let mut menus_ok = menus.iter().map(|m| text(&m["label"])).collect::<Vec<_>>() == MENUS;
    for menu in &menus {
        let entries = menu["items"].as_array().cloned().unwrap_or_default();
        menus_ok &= menu["expanded"] == "true" && !entries.is_empty();
        for entry in &entries {
            let action = entry["action"].as_str();
            menus_ok &= if entry["disabled"] == true {
                action.is_none() && !text(&entry["reason"]).trim().is_empty()
            } else {
                action.is_some_and(|a| MENU_ACTIONS.contains(&a))
                    && !text(&entry["label"]).is_empty()
            };
            if let (false, Some(a)) = (entry["disabled"] == true, action) {
                enabled_actions.push(a.to_owned());
            }
        }
    }
    let unique = {
        let mut sorted = enabled_actions.clone();
        sorted.sort_unstable();
        sorted.dedup();
        sorted.len() == enabled_actions.len()
    };
    let topbar_ok = kinds == TOPBAR_ORDER
        && item("logo").is_some_and(|i| i["text"] == "herdr")
        && menu_texts == MENUS
        && item("search").is_some_and(|i| text(&i["text"]).contains(SEARCH_TEXT))
        && item("search-hint").is_some_and(|i| i["text"] == SEARCH_HINT)
        && item("host").is_some_and(|i| i["text"] == endpoint)
        && host["dot"] == true
        && host["text"] == endpoint
        && host["text_overflow"] == "ellipsis"
        && host["white_space"] == "nowrap"
        && text(&host["title"]).starts_with(endpoint)
        && icons_ok
        && menus_ok
        && !enabled_actions.is_empty()
        && unique;

    // AC-010-02 — status bar against the engine.
    let status = frame["status_items"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let status_text = |kind: &str| {
        status
            .iter()
            .find(|s| s["item"] == kind)
            .map(|s| text(&s["text"]).to_owned())
    };
    let plural = |n: u64, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    let panes_in_tab = engine["pane_ids_in_tab"]
        .as_array()
        .map(Vec::len)
        .unwrap_or(0) as u64;
    let counts = format!(
        "{} · {}",
        plural(panes_in_tab, "pane", "panes"),
        plural(engine["tabs"].as_u64().unwrap_or(0), "aba", "abas")
    );
    let status_ok = status
        .first()
        .is_some_and(|s| s["item"] == "server" && s["phase"] == "live")
        && status_text("server")
            == Some(format!(
                "herdr server {} · conectado",
                text(&engine["version"])
            ))
        && status_text("host").as_deref() == Some(endpoint)
        && engine["branch"] == FIXTURE_BRANCH
        && status_text("branch").as_deref() == Some(text(&engine["branch"]))
        && status_text("counts") == Some(counts)
        && status_text("channel") == Some(format!("canal {}", text(&engine["channel"])))
        && panes_in_tab > 0;

    // AC-010-02 — contrast of small text, recomputed here: the bars, every open menu and the palette.
    let outside = field(report, "ctrlk_outside", "report")?;
    let texts_of = |v: &Value| v.as_array().cloned().unwrap_or_default();
    let mut texts = texts_of(&frame["small_texts"]);
    for menu in &menus {
        texts.extend(texts_of(&menu["small_texts"]));
    }
    texts.extend(texts_of(&outside["palette"]["small_texts"]));
    let small: Vec<&Value> = texts
        .iter()
        .filter(|t| t["visibility"] == "visible")
        .filter(|t| {
            let size = num(&t["font_size"]).unwrap_or(0.0);
            let weight = num(&t["font_weight"]).unwrap_or(400.0);
            !(size >= 24.0 || (size >= 18.66 && weight >= 700.0))
        })
        .collect();
    let has = |scope: &str, needle: &str| {
        small
            .iter()
            .any(|t| t["scope"] == scope && text(&t["text"]).contains(needle))
    };
    // The page keeps the first 60 characters of each text.
    let shown = |scope: &str, label: &str| {
        let want: String = label.chars().take(60).collect();
        !want.is_empty()
            && small
                .iter()
                .any(|t| t["scope"] == scope && text(&t["text"]) == want)
    };
    let palette_sections = outside["palette"]["sections"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    // Every menu item label measured with its menu open; every palette title and entry label
    // measured with the palette open.
    let menus_measured = !menus.is_empty()
        && menus.iter().all(|m| {
            let scope = format!("menu:{}", text(&m["label"]));
            let entries = m["items"].as_array().cloned().unwrap_or_default();
            !entries.is_empty() && entries.iter().all(|e| shown(&scope, text(&e["label"])))
        });
    let palette_measured = !palette_sections.is_empty()
        && palette_sections.iter().all(|s| {
            let title = text(&s["title"]).to_lowercase();
            small
                .iter()
                .any(|t| t["scope"] == "palette" && text(&t["text"]).to_lowercase() == title)
                && s["entries"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .iter()
                    .all(|e| shown("palette", text(&e["label"])))
        });
    let contrast_ok = menus_measured
        && palette_measured
        && has("topbar", "herdr")
        && has("topbar", "Arquivo")
        && has("topbar", SEARCH_HINT)
        && has("topbar", endpoint)
        && has("status", "herdr server")
        && has("status", "canal")
        && small.iter().all(|t| {
            contrast(
                text(&t["color"]),
                text(&t["background"]),
                num(&t["opacity"]).unwrap_or(0.0),
            )
            .is_some_and(|r| r >= MIN_SMALL_TEXT_RATIO)
        });

    // AC-010-03 — palette outside the terminal.
    let opener = text(&outside["opener"]);
    let sections = palette_sections;
    let titles: Vec<&str> = sections.iter().map(|s| text(&s["title"])).collect();
    let ids = |title: &str| -> Vec<String> {
        sections
            .iter()
            .find(|s| s["title"] == title)
            .and_then(|s| s["entries"].as_array())
            .map(|e| e.iter().map(|x| text(&x["id"]).to_owned()).collect())
            .unwrap_or_default()
    };
    let mut pane_entries = ids("Panes");
    pane_entries.sort_unstable();
    let want_panes: Vec<String> = engine["pane_ids_in_tab"]
        .as_array()
        .map(|p| p.iter().map(|x| format!("pane:{}", text(x))).collect())
        .unwrap_or_default();
    // Agentes equal to the engine's agent list and Projetos equal to the window catalog, both read
    // by the parent when the palette opened; an empty source is never a pass.
    let outside_answer = answer("visual-ctrlk-outside")?;
    let want_agents = engine_agent_ids(&outside_answer["engine_agent_list"]).unwrap_or_default();
    let want_projects = catalog_project_ids(&outside_answer["project_catalog"]).unwrap_or_default();
    let mut agent_entries = ids("Agentes");
    agent_entries.sort_unstable();
    let mut project_entries = ids("Projetos");
    project_entries.sort_unstable();
    let palette_ok = opener == "host"
        && one_trusted_key(&outside["keys"], "host", true)
        && outside["palette"]["input_focused"] == true
        && titles == PALETTE_SECTIONS
        && pane_entries == want_panes
        && !want_agents.is_empty()
        && agent_entries == want_agents
        && !want_projects.is_empty()
        && project_entries == want_projects
        && !ids("Comandos").is_empty()
        && outside_answer["pty_hex"] == "";

    // AC-010-03 — Escape.
    let escape = field(report, "escape", "report")?;
    let escape_ok = escape["palette"].is_null()
        && !opener.is_empty()
        && escape["focus_after"] == opener
        && answer("visual-escape")?["pty_hex"] == "";

    // AC-010-03 — Ctrl+K in the terminal reaches the PTY once.
    let terminal = field(report, "ctrlk_terminal", "report")?;
    let terminal_ok = terminal["focused"] == true
        && terminal["palette"].is_null()
        && one_trusted_key(&terminal["keys"], TERMINAL_TARGET, false)
        && answer("visual-ctrlk-terminal")?["pty_hex"] == "0b";

    Ok(vec![
        (CHECKS[0], dims_ok),
        (CHECKS[1], tokens_ok),
        (CHECKS[2], fonts_ok),
        (CHECKS[3], topbar_ok),
        (CHECKS[4], status_ok),
        (CHECKS[5], contrast_ok),
        (CHECKS[6], palette_ok),
        (CHECKS[7], escape_ok),
        (CHECKS[8], terminal_ok),
    ])
}
