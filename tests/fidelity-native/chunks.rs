//! Lazy editor probe of AC-007-03 in the composed window.
//!
//! The page reports script pathnames it has fetched (resource timing entries, including those a
//! buffered `PerformanceObserver` saw, plus script/modulepreload elements) at three moments:
//! mount stable, after ≥ 2000 ms idle, and after opening a file. The build's chunk map
//! (`dist/.vite/module-chunks.json`, written by `vite.config.ts`) maps them to source modules.
//! An unknown script fails instead of counting zero (stale or foreign map), and the positive
//! control must detect editor modules, so a dead probe cannot pass.

use std::collections::BTreeMap;

use serde_json::Value;

/// Minimum idle window between the mount and the idle probe.
pub const IDLE_WINDOW_MS: f64 = 2000.0;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkMap {
    chunks: BTreeMap<String, Vec<String>>,
}

impl ChunkMap {
    pub fn parse(raw: &str) -> Result<Self, String> {
        let value: Value = serde_json::from_str(raw).map_err(|e| format!("not JSON: {e}"))?;
        if value["version"] != 1 {
            return Err(format!("unsupported version {}", value["version"]));
        }
        let chunks: BTreeMap<String, Vec<String>> = serde_json::from_value(value["chunks"].clone())
            .map_err(|e| format!("chunks: file -> modules expected: {e}"))?;
        if chunks.is_empty() {
            return Err("chunk map lists no chunk".into());
        }
        Ok(Self { chunks })
    }

    /// Source modules of the loaded scripts (sorted, deduplicated).
    pub fn modules(&self, scripts: &[String]) -> Result<Vec<String>, String> {
        if scripts.is_empty() {
            return Err("probe saw no script".into());
        }
        let mut all = Vec::new();
        for script in scripts {
            let modules = self
                .chunks
                .get(script.trim_start_matches('/'))
                .ok_or_else(|| format!("loaded script {script} is not a chunk of this build"))?;
            all.extend(modules.iter().cloned());
        }
        all.sort();
        all.dedup();
        Ok(all)
    }
}

/// Editor and language modules: CodeMirror/Lezer and their private dependencies plus the lazy
/// editor entry in `src/editor/` (same classification as the isolated proof of spec 005).
pub fn is_editor_module(module: &str) -> bool {
    const PACKAGES: [&str; 6] = [
        "node_modules/@codemirror/",
        "node_modules/@lezer/",
        "node_modules/@marijn/find-cluster-break/",
        "node_modules/style-mod/",
        "node_modules/w3c-keyname/",
        "node_modules/crelt/",
    ];
    module.starts_with("src/editor/") || PACKAGES.iter().any(|p| module.contains(p))
}

#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub at_ms: f64,
    pub scripts: Vec<String>,
    pub editor_dom: bool,
}

impl Snapshot {
    pub fn from_json(value: &Value) -> Result<Self, String> {
        let at_ms = value["at_ms"]
            .as_f64()
            .ok_or_else(|| format!("snapshot without at_ms: {value}"))?;
        let scripts = value["scripts"]
            .as_array()
            .ok_or_else(|| format!("snapshot without scripts: {value}"))?
            .iter()
            .map(|s| {
                s.as_str()
                    .map(str::to_owned)
                    .ok_or("script is not a string")
            })
            .collect::<Result<Vec<_>, _>>()?;
        let editor_dom = value["editor_dom"]
            .as_bool()
            .ok_or_else(|| format!("snapshot without editor_dom: {value}"))?;
        Ok(Self {
            at_ms,
            scripts,
            editor_dom,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LazyEditorVerdict {
    pub idle_window_ms: f64,
    pub editor_modules_mounted: Vec<String>,
    pub editor_modules_idle: Vec<String>,
    pub editor_modules_opened: Vec<String>,
}

impl LazyEditorVerdict {
    /// Checks named as in `plan::FLOW` for the `lazy-editor` phase.
    pub fn checks(&self) -> Vec<(&'static str, bool)> {
        vec![
            (
                "idle_window_at_least_2000_ms",
                self.idle_window_ms >= IDLE_WINDOW_MS,
            ),
            (
                "zero_editor_modules_mounted",
                self.editor_modules_mounted.is_empty(),
            ),
            (
                "zero_editor_modules_idle",
                self.editor_modules_idle.is_empty(),
            ),
            (
                "editor_modules_detected_after_open",
                !self.editor_modules_opened.is_empty(),
            ),
        ]
    }
}

/// Maps the three snapshots. Structural errors (unknown script, clock going back, editor DOM
/// before opening) are errors, not a verdict.
pub fn lazy_editor_verdict(
    map: &ChunkMap,
    mounted: &Snapshot,
    idle: &Snapshot,
    opened: &Snapshot,
) -> Result<LazyEditorVerdict, String> {
    if !(mounted.at_ms <= idle.at_ms && idle.at_ms < opened.at_ms) {
        return Err(format!(
            "snapshots out of order: mounted {} idle {} opened {}",
            mounted.at_ms, idle.at_ms, opened.at_ms
        ));
    }
    if mounted.editor_dom || idle.editor_dom {
        return Err("editor DOM present before opening a file".into());
    }
    if !opened.editor_dom {
        return Err("positive control: no editor DOM after opening a file".into());
    }
    let editor = |snapshot: &Snapshot| -> Result<Vec<String>, String> {
        Ok(map
            .modules(&snapshot.scripts)?
            .into_iter()
            .filter(|m| is_editor_module(m))
            .collect())
    };
    Ok(LazyEditorVerdict {
        idle_window_ms: idle.at_ms - mounted.at_ms,
        editor_modules_mounted: editor(mounted)?,
        editor_modules_idle: editor(idle)?,
        editor_modules_opened: editor(opened)?,
    })
}
