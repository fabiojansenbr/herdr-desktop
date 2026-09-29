//! WebKit settings API diagnostic of the resource-bench window (spec 007 idle memory; test
//! harness only, never a product default).
//!
//! A closed set of arms selected by `params.webkit_policy` of the `resource-bench` phase only:
//!
//! - `control`: getters only, no setter;
//! - `api-never`: `hardware-acceleration-policy = never`;
//! - `api-software`: `never` + `enable-webgl = false` + `enable-2d-canvas-acceleration = false`
//!   (the 2.46 property; the deprecated `enable-accelerated-2d-canvas` is a no-op on 2.52 and is
//!   only read back).
//!
//! The harness plugin hook (`window.rs`, `on_webview_ready` → `Webview::with_webview`) applies the
//! arm to the real Wry WebView and writes the getter readback to `<report>.policy.json`. The bench
//! runner judges the arm only by that readback ([`validate`]); the request is never proof.
//!
//! Timing limit: the hook runs after Wry constructed the WebView and called `load_uri`, so the
//! WebProcess and GL/EGL may already be initialized before the setter; only the measurement decides
//! whether the API changes memory.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

/// Params key of the resource-bench window.
pub const PARAM: &str = "webkit_policy";
/// The only phase that accepts [`PARAM`].
pub const PHASE: &str = "resource-bench";
/// Schema of the readback record.
pub const SCHEMA: &str = "herdr-desktop.webkit-policy.v1";
pub const PROP_POLICY: &str = "hardware-acceleration-policy";
pub const PROP_WEBGL: &str = "enable-webgl";
/// WebKitGTK ≥ 2.46 2D canvas acceleration property (installed GIR WebKit2-4.1, default TRUE).
pub const PROP_CANVAS: &str = "enable-2d-canvas-acceleration";
/// Deprecated since 2.32 (no-op on the Skia renderer); read back only, never set.
pub const PROP_CANVAS_DEPRECATED: &str = "enable-accelerated-2d-canvas";
/// Properties every record must read back.
pub const REQUIRED: [&str; 3] = [PROP_POLICY, PROP_WEBGL, PROP_CANVAS];
/// Where and when the hook runs (recorded verbatim).
pub const HOOK: &str = "tauri plugin on_webview_ready -> Webview::with_webview (main thread), after wry built the WebView and called load_uri";
pub const HOOK_LIMIT: &str = "WebView construction and the first load may initialize the WebProcess and GL/EGL before the setter; getters read the UI-process WebKitSettings, not WebProcess internals";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Policy {
    Control,
    ApiNever,
    ApiSoftware,
}

impl Policy {
    pub fn parse(name: &str) -> Result<Self, String> {
        match name {
            "control" => Ok(Self::Control),
            "api-never" => Ok(Self::ApiNever),
            "api-software" => Ok(Self::ApiSoftware),
            other => Err(format!(
                "{PARAM} must be control|api-never|api-software, got {other:?}"
            )),
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Control => "control",
            Self::ApiNever => "api-never",
            Self::ApiSoftware => "api-software",
        }
    }

    /// Exact `(property, value)` setters of the arm, in application order.
    pub fn setters(&self) -> &'static [(&'static str, &'static str)] {
        match self {
            Self::Control => &[],
            Self::ApiNever => &[(PROP_POLICY, "never")],
            Self::ApiSoftware => &[
                (PROP_POLICY, "never"),
                (PROP_WEBGL, "false"),
                (PROP_CANVAS, "false"),
            ],
        }
    }
}

/// Arm of a window phase: `None` without [`PARAM`] (the standard native flow is untouched);
/// the key outside [`PHASE`] or an unknown/non-string value is refused.
pub fn selection(phase: &str, params: &Value) -> Result<Option<Policy>, String> {
    let Some(value) = params.get(PARAM) else {
        return Ok(None);
    };
    if phase != PHASE {
        return Err(format!(
            "{PARAM} is only accepted in phase {PHASE}, not {phase:?}"
        ));
    }
    let name = value
        .as_str()
        .ok_or_else(|| format!("{PARAM} must be a string, got {value}"))?;
    Policy::parse(name).map(Some)
}

/// Readback record next to the owned report (`report.json` → `report.policy.json`).
pub fn record_path(report_path: &Path) -> PathBuf {
    report_path.with_extension("policy.json")
}

fn text(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// Setters of a valid readback split by whether the getter value actually changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Effect {
    /// Set properties whose getter differed before and reads the requested value after.
    pub changed: Vec<&'static str>,
    /// Set properties that already had the requested value (idempotent; no proof of effect).
    pub no_op: Vec<&'static str>,
}

impl Effect {
    /// True only when at least one setter changed a getter value.
    pub fn proven(&self) -> bool {
        !self.changed.is_empty()
    }
}

/// Judges one window's readback record for `policy`: same schema/phase/arm/pid, one WebView, no
/// error or unsupported property, every required getter read, exactly the arm's setters, each set
/// property reading back exactly the requested value after, and every other required property
/// unchanged. A setter whose property already had the value is valid but reported as `no_op`
/// (ESC31), never as `changed`.
pub fn validate(policy: Policy, record: &Value, window_pid: u32) -> Result<Effect, String> {
    if !record.is_object() {
        return Err("no webkit policy readback record".into());
    }
    let expect = |key: &str, want: Value| {
        if record[key] == want {
            Ok(())
        } else {
            Err(format!("{key} is {} not {want}", record[key]))
        }
    };
    expect("schema", json!(SCHEMA))?;
    expect("phase", json!(PHASE))?;
    expect("variant", json!(policy.name()))?;
    expect("pid", json!(window_pid))?;
    expect("webview_count", json!(1))?;
    expect("errors", json!([]))?;
    expect("unsupported", json!([]))?;
    let setters: Vec<String> = policy
        .setters()
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect();
    expect("setters", json!(setters))?;
    let mut effect = Effect {
        changed: Vec::new(),
        no_op: Vec::new(),
    };
    for prop in REQUIRED {
        if record["available"][prop] != json!(true) {
            return Err(format!("property {prop} not available"));
        }
        let before = text(&record["before"][prop])
            .ok_or_else(|| format!("getter {prop} before not read"))?;
        let after =
            text(&record["after"][prop]).ok_or_else(|| format!("getter {prop} after not read"))?;
        match policy.setters().iter().find(|(k, _)| *k == prop) {
            Some((_, want)) => {
                if after != *want {
                    return Err(format!("{prop} reads {after} after setting {want}"));
                }
                if before == *want {
                    effect.no_op.push(prop);
                } else {
                    effect.changed.push(prop);
                }
            }
            None => {
                if after != before {
                    return Err(format!(
                        "{prop} changed from {before} to {after} without a setter"
                    ));
                }
            }
        }
    }
    Ok(effect)
}

#[cfg(target_os = "linux")]
mod apply {
    use super::*;
    use webkit2gtk::glib::prelude::ObjectExt;
    use webkit2gtk::{HardwareAccelerationPolicy, Settings, SettingsExt, WebViewExt};

    fn policy_name(policy: HardwareAccelerationPolicy) -> String {
        match policy {
            HardwareAccelerationPolicy::OnDemand => "on-demand".into(),
            HardwareAccelerationPolicy::Always => "always".into(),
            HardwareAccelerationPolicy::Never => "never".into(),
            other => format!("unknown:{other:?}"),
        }
    }

    fn is_bool(settings: &Settings, prop: &str) -> bool {
        settings
            .find_property(prop)
            .is_some_and(|spec| spec.value_type() == bool::static_type())
    }

    use webkit2gtk::glib::types::StaticType;

    fn available(settings: &Settings) -> Value {
        json!({
            PROP_POLICY: settings.find_property(PROP_POLICY).is_some(),
            PROP_WEBGL: is_bool(settings, PROP_WEBGL),
            PROP_CANVAS: is_bool(settings, PROP_CANVAS),
            PROP_CANVAS_DEPRECATED: is_bool(settings, PROP_CANVAS_DEPRECATED),
        })
    }

    fn getters(settings: &Settings) -> Value {
        let boolean = |prop: &str| {
            if is_bool(settings, prop) {
                json!(settings.property::<bool>(prop))
            } else {
                Value::Null
            }
        };
        json!({
            PROP_POLICY: if settings.find_property(PROP_POLICY).is_some() {
                json!(policy_name(settings.hardware_acceleration_policy()))
            } else {
                Value::Null
            },
            PROP_WEBGL: boolean(PROP_WEBGL),
            PROP_CANVAS: boolean(PROP_CANVAS),
            PROP_CANVAS_DEPRECATED: boolean(PROP_CANVAS_DEPRECATED),
        })
    }

    /// Applies `policy` to the real WebView and returns the readback record. A missing required
    /// property is reported as unsupported and no setter of the arm runs.
    pub fn apply(
        webview: &webkit2gtk::WebView,
        policy: Policy,
        webview_label: &str,
        webview_count: usize,
        elapsed_ms: u128,
    ) -> Value {
        let mut errors: Vec<String> = Vec::new();
        let mut unsupported: Vec<&str> = Vec::new();
        let mut setters: Vec<String> = Vec::new();
        let hook = json!({
            "path": HOOK,
            "limit": HOOK_LIMIT,
            "webview_label": webview_label,
            "is_loading": webview.is_loading(),
            "estimated_load_progress": webview.estimated_load_progress(),
            "elapsed_ms_since_window_start": elapsed_ms as u64,
        });
        let Some(settings) = WebViewExt::settings(webview) else {
            return json!({
                "schema": SCHEMA, "phase": PHASE, "variant": policy.name(),
                "pid": std::process::id(), "webview_count": webview_count, "hook": hook,
                "available": {}, "before": {}, "setters": [], "after": {},
                "unsupported": [], "errors": ["webkit_web_view_get_settings returned NULL"],
            });
        };
        let avail = available(&settings);
        let before = getters(&settings);
        for prop in REQUIRED {
            if avail[prop] != json!(true) {
                unsupported.push(prop);
            }
        }
        if unsupported.is_empty() {
            for (prop, value) in policy.setters() {
                match (*prop, *value) {
                    (PROP_POLICY, "never") => {
                        settings.set_hardware_acceleration_policy(HardwareAccelerationPolicy::Never)
                    }
                    (PROP_WEBGL, "false") => settings.set_enable_webgl(false),
                    (PROP_CANVAS, "false") => settings.set_property(PROP_CANVAS, false),
                    other => {
                        errors.push(format!("no adapter for setter {other:?}"));
                        continue;
                    }
                }
                setters.push(format!("{prop}={value}"));
            }
        }
        let after = getters(&settings);
        json!({
            "schema": SCHEMA,
            "phase": PHASE,
            "variant": policy.name(),
            "pid": std::process::id(),
            "webview_count": webview_count,
            "hook": hook,
            "available": avail,
            "before": before,
            "setters": setters,
            "after": after,
            "unsupported": unsupported,
            "errors": errors,
        })
    }
}

#[cfg(target_os = "linux")]
pub use apply::apply;
