//! Matrix and run modes of the resource bench.

/// Output of the synthetic producers, as seen by the GUI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Output {
    /// Producers idle (baseline / idle comparison).
    Idle,
    /// Terminal activity shown while every producer emits.
    Visible,
    /// Files activity shown (no file opened), terminal hidden by the UI, producers emitting.
    Hidden,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scenario {
    /// Real PTYs in the disposable session (15 = 1 active + 14 in background tabs/workspaces).
    pub panes: u8,
    /// Independent App processes attached to the same engine and session.
    pub clients: u8,
    pub output: Output,
}

pub struct Matrix {
    pub cells: Vec<Scenario>,
    pub idle_baseline: Scenario,
    pub idle_comparison: Scenario,
}

/// Fixed synthetic output per PTY (every pane, including hidden tabs): `SYNTHETIC_LINE`
/// written `SYNTHETIC_LINES_PER_S` times per second by a generator process in the pane.
pub const SYNTHETIC_LINE: &str =
    "herdr-bench 0123456789 abcdefghijklmnopqrstuvwxyz ABCDEFGHIJKLMNOPQRSTUVWXYZ .\n";
pub const SYNTHETIC_LINES_PER_S: u64 = 50;
/// Bytes/s one producer must really emit (checked from its own byte counter).
pub const fn producer_bytes_per_s() -> u64 {
    SYNTHETIC_LINE.len() as u64 * SYNTHETIC_LINES_PER_S
}
/// Accepted deviation of the measured producer rate (per mille).
pub const RATE_TOLERANCE_PERMILLE: u64 = 100;

pub const MEMORY_CHECKPOINTS_S: [u64; 4] = [0, 600, 1200, 1800];
pub const SMOKE_MAX_S: f64 = 3.0;
pub const ACCEPT_MIN_WARMUP_S: f64 = 5.0;
pub const ACCEPT_MIN_MEASURED_S: f64 = 60.0;

pub fn matrix() -> Matrix {
    let mut cells = Vec::new();
    for panes in [1, 15] {
        for clients in [1, 2] {
            for output in [Output::Visible, Output::Hidden] {
                cells.push(Scenario {
                    panes,
                    clients,
                    output,
                });
            }
        }
    }
    Matrix {
        cells,
        idle_baseline: Scenario {
            panes: 1,
            clients: 1,
            output: Output::Idle,
        },
        idle_comparison: Scenario {
            panes: 15,
            clients: 1,
            output: Output::Idle,
        },
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Mode {
    /// Topology/isolation smoke; always INSUFFICIENT_FOR_ACCEPTANCE.
    Smoke {
        measured_s: f64,
    },
    Acceptance {
        warmup_s: f64,
        measured_s: f64,
    },
    /// Same GUI kept for 1800 s of real time after use; samples at [`MEMORY_CHECKPOINTS_S`].
    Memory,
}

impl Mode {
    pub fn smoke(measured_s: f64) -> Result<Self, String> {
        if measured_s.is_finite() && measured_s > 0.0 && measured_s <= SMOKE_MAX_S {
            Ok(Self::Smoke { measured_s })
        } else {
            Err(format!(
                "smoke duration must be in (0, {SMOKE_MAX_S}] s, got {measured_s}"
            ))
        }
    }

    pub fn acceptance(warmup_s: f64, measured_s: f64) -> Result<Self, String> {
        // Finite first: an infinite value orders above any minimum and would never end.
        if !(warmup_s.is_finite() && warmup_s >= ACCEPT_MIN_WARMUP_S) {
            return Err(format!(
                "warmup {warmup_s} s must be finite and >= {ACCEPT_MIN_WARMUP_S} s"
            ));
        }
        if !(measured_s.is_finite() && measured_s >= ACCEPT_MIN_MEASURED_S) {
            return Err(format!(
                "measured {measured_s} s must be finite and >= {ACCEPT_MIN_MEASURED_S} s"
            ));
        }
        Ok(Self::Acceptance {
            warmup_s,
            measured_s,
        })
    }

    pub fn memory() -> Self {
        Self::Memory
    }

    pub fn warmup_s(&self) -> f64 {
        match self {
            Self::Smoke { .. } | Self::Memory => 0.0,
            Self::Acceptance { warmup_s, .. } => *warmup_s,
        }
    }

    pub fn measured_s(&self) -> f64 {
        match self {
            Self::Smoke { measured_s } | Self::Acceptance { measured_s, .. } => *measured_s,
            Self::Memory => *MEMORY_CHECKPOINTS_S.last().unwrap() as f64,
        }
    }
}

/// Seconds the resource-bench window must stay alive: one collector window per load output
/// plus slack for identity/settle. Memory is 1800 s of one idle GUI; 150 s is only enough for smoke.
pub fn resource_window_timeout_s(mode: &Mode, load: Load) -> u64 {
    let windows = load.outputs().len() as f64;
    let per = mode.warmup_s() + mode.measured_s() + 5.0;
    (120.0 + windows * per).ceil() as u64
}

/// Live resource measurement of this spec: smoke, acceptance and memory of any matrix topology.
pub fn resource_live_authorised(run: &RunSpec) -> Result<(), String> {
    match run.args.mode {
        Mode::Smoke { .. } | Mode::Acceptance { .. } | Mode::Memory => Ok(()),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RunArgs {
    pub mode: Mode,
    pub out: std::path::PathBuf,
}

/// `--mode smoke|acceptance|memory --duration S [--warmup S] --out ABS`; nothing implicit.
pub fn parse_args(args: &[&str]) -> Result<RunArgs, String> {
    let mut values = std::collections::BTreeMap::new();
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        match *flag {
            "--mode" | "--duration" | "--warmup" | "--out" => {
                let value = it.next().ok_or_else(|| format!("{flag} needs a value"))?;
                if values.insert(*flag, *value).is_some() {
                    return Err(format!("{flag} given twice"));
                }
            }
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    let number = |key: &str| -> Result<f64, String> {
        values
            .get(key)
            .ok_or_else(|| format!("{key} is required"))?
            .parse::<f64>()
            .map_err(|e| format!("{key}: {e}"))
    };
    let mode = match values.get("--mode").copied() {
        Some("smoke") => Mode::smoke(number("--duration")?)?,
        Some("acceptance") => Mode::acceptance(number("--warmup")?, number("--duration")?)?,
        Some("memory") => {
            if number("--duration")? != Mode::Memory.measured_s() {
                return Err("memory mode runs exactly 1800 s of real time".into());
            }
            Mode::Memory
        }
        other => {
            return Err(format!(
                "--mode must be smoke|acceptance|memory, got {other:?}"
            ))
        }
    };
    let out = std::path::PathBuf::from(values.get("--out").ok_or("--out is required")?);
    if !out.is_absolute() {
        return Err("--out must be absolute".into());
    }
    Ok(RunArgs { mode, out })
}

/// Tolerated excess of a producer counter window over the measured window (reading latency after
/// the collector exits). A window that also covers the warmup exceeds it.
pub const COUNTER_WINDOW_SLACK_S: f64 = 1.0;

/// Page ↔ parent steps of `src/features/fidelity/resource-bench.ts`, in protocol order.
pub const BENCH_STEPS: [&str; 5] = [
    "bench-ready",
    "bench-visible",
    "bench-hidden",
    "bench-idle",
    "bench-stop",
];

/// Common measurement barrier of N clients: the next unhandled step is released only when every
/// client requested it (one collector run for all GUIs, never sequential runs). `bench-stop` is
/// released as soon as any client asks, so one failing client never strands the other; clients
/// that ask different windows (or unknown steps) are an error.
pub fn barrier_step(pending: &[&[&str]], handled: &[&str]) -> Result<Option<&'static str>, String> {
    for client in pending {
        if let Some(bad) = client.iter().find(|s| !BENCH_STEPS.contains(s)) {
            return Err(format!("unknown bench step {bad:?}"));
        }
    }
    if pending.iter().any(|c| c.contains(&"bench-stop")) && !handled.contains(&"bench-stop") {
        return Ok(Some("bench-stop"));
    }
    let next: Vec<Option<&str>> = pending
        .iter()
        .map(|c| c.iter().copied().find(|s| !handled.contains(s)))
        .collect();
    let requested: std::collections::BTreeSet<&str> = next.iter().flatten().copied().collect();
    if requested.len() > 1 {
        return Err(format!("clients requested different steps {requested:?}"));
    }
    match requested.into_iter().next() {
        Some(step) if next.iter().all(Option::is_some) => {
            Ok(BENCH_STEPS.iter().copied().find(|s| *s == step))
        }
        _ => Ok(None),
    }
}

/// Collector group of the `index`-th PTY (1-based): `pty` for one pane, `pty_<index>` otherwise.
pub fn pty_group(index: u8, panes: u8) -> String {
    if panes == 1 {
        "pty".into()
    } else {
        format!("pty_{index}")
    }
}

/// Background PTYs as (tabs in the active workspace, extra workspaces): 1 active + the rest.
pub fn background_split(panes: u8) -> (u8, u8) {
    let background = panes.saturating_sub(1);
    (background / 2, background - background / 2)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Load {
    /// No generator at all: the idle cell.
    Idle,
    /// Generators at the fixed rate: visible then Files-hidden windows in the same clients.
    Output,
}

impl Load {
    pub fn outputs(&self) -> Vec<Output> {
        match self {
            Self::Idle => vec![Output::Idle],
            Self::Output => vec![Output::Visible, Output::Hidden],
        }
    }
}

/// WebKitGTK renderer variant of the private window (memory A/B diagnostic only). A closed set of
/// names, each mapped to at most one fixed variable; never a product setting and never generic
/// environment injection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RendererVariant {
    /// Neither variable: the harness renderer as measured by the baseline.
    Control,
    /// `WEBKIT_DISABLE_COMPOSITING_MODE=1`.
    NoCompositing,
    /// `WEBKIT_SKIA_ENABLE_CPU_RENDERING=1`.
    CpuSkia,
    /// `GDK_DEBUG=nogl` (GTK3: GDK behaves as if GL were unavailable; needs GTK debug support).
    GdkNoGl,
    /// `GDK_RENDERING=image` (GTK3: image backing surfaces, no GTK hardware acceleration).
    GdkImage,
    /// No variable; WebKit settings API `hardware-acceleration-policy=never` applied by the
    /// harness hook and judged by its getter readback.
    ApiNever,
    /// No variable; API `never` + `enable-webgl=false` + `enable-2d-canvas-acceleration=false`.
    ApiSoftware,
}

/// Exact `(key, value)` pairs a renderer variant may add to the window launch. Only these
/// pairs; any other value of the same key is refused.
pub const RENDERER_VARIANT_PAIRS: [(&str, &str); 4] = [
    ("WEBKIT_DISABLE_COMPOSITING_MODE", "1"),
    ("WEBKIT_SKIA_ENABLE_CPU_RENDERING", "1"),
    ("GDK_DEBUG", "nogl"),
    ("GDK_RENDERING", "image"),
];
/// Keys of [`RENDERER_VARIANT_PAIRS`], read back from the window environ.
pub const RENDERER_VARIANT_KEYS: [&str; 4] = [
    RENDERER_VARIANT_PAIRS[0].0,
    RENDERER_VARIANT_PAIRS[1].0,
    RENDERER_VARIANT_PAIRS[2].0,
    RENDERER_VARIANT_PAIRS[3].0,
];
/// Isolation variable every variant keeps (set by the private display launch, never by a variant).
pub const RENDERER_FIXED: (&str, &str) = ("WEBKIT_DISABLE_DMABUF_RENDERER", "1");

impl RendererVariant {
    pub fn parse(name: &str) -> Result<Self, String> {
        match name {
            "control" => Ok(Self::Control),
            "no-compositing" => Ok(Self::NoCompositing),
            "cpu-skia" => Ok(Self::CpuSkia),
            "gdk-nogl" => Ok(Self::GdkNoGl),
            "gdk-image" => Ok(Self::GdkImage),
            "api-never" => Ok(Self::ApiNever),
            "api-software" => Ok(Self::ApiSoftware),
            other => Err(format!(
                "--renderer must be control|no-compositing|cpu-skia|gdk-nogl|gdk-image|api-never|api-software, got {other:?}"
            )),
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Control => "control",
            Self::NoCompositing => "no-compositing",
            Self::CpuSkia => "cpu-skia",
            Self::GdkNoGl => "gdk-nogl",
            Self::GdkImage => "gdk-image",
            Self::ApiNever => "api-never",
            Self::ApiSoftware => "api-software",
        }
    }

    pub fn env(&self) -> &'static [(&'static str, &'static str)] {
        match self {
            Self::Control => &[],
            Self::NoCompositing => &[("WEBKIT_DISABLE_COMPOSITING_MODE", "1")],
            Self::CpuSkia => &[("WEBKIT_SKIA_ENABLE_CPU_RENDERING", "1")],
            Self::GdkNoGl => &[("GDK_DEBUG", "nogl")],
            Self::GdkImage => &[("GDK_RENDERING", "image")],
            Self::ApiNever | Self::ApiSoftware => &[],
        }
    }

    /// WebKit settings API arm passed to the window params; every env arm is the getter-only
    /// control. Linux only: the window module owns the policy adapter.
    #[cfg(target_os = "linux")]
    pub fn webkit_policy(&self) -> crate::support::window::webkit_policy::Policy {
        use crate::support::window::webkit_policy::Policy;
        match self {
            Self::ApiNever => Policy::ApiNever,
            Self::ApiSoftware => Policy::ApiSoftware,
            _ => Policy::Control,
        }
    }
}

/// Guard applied before a variant reaches the window launch: at most one variable, from
/// [`RENDERER_VARIANT_PAIRS`], with exactly that pair's value. Display, socket and DMA-BUF keys
/// are refused.
pub fn check_renderer_env(env: &[(&str, &str)]) -> Result<(), String> {
    if env.len() > 1 {
        return Err(format!(
            "a renderer variant sets at most one variable: {env:?}"
        ));
    }
    for (k, v) in env {
        if !RENDERER_VARIANT_PAIRS.contains(&(*k, *v)) {
            return Err(format!("renderer variant may not set {k}={v}"));
        }
    }
    Ok(())
}

/// True when the WebKit renderer variables read from the window's own environ are exactly
/// [`RENDERER_FIXED`] plus the variant's variable (no other variant key present).
pub fn variant_applied(
    variant: RendererVariant,
    window_env: &std::collections::BTreeMap<String, String>,
) -> bool {
    let mut expected: std::collections::BTreeMap<String, String> = variant
        .env()
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect();
    expected.insert(RENDERER_FIXED.0.to_owned(), RENDERER_FIXED.1.to_owned());
    *window_env == expected
}

#[derive(Debug, Clone, PartialEq)]
pub struct RunSpec {
    pub args: RunArgs,
    pub panes: u8,
    pub clients: u8,
    pub load: Load,
    /// Optional `--renderer`; defaults to [`RendererVariant::Control`].
    pub renderer: RendererVariant,
}

impl RunSpec {
    /// Background PTYs as (tabs in the active workspace, extra workspaces): 1 active + 14.
    pub fn background_split(&self) -> (u8, u8) {
        background_split(self.panes)
    }
}

/// [`parse_args`] plus the required topology `--panes 1|15 --clients 1|2 --load idle|output` and
/// the optional `--renderer control|no-compositing|cpu-skia|gdk-nogl|gdk-image|api-never|api-software`
/// (default control).
pub fn parse_run(args: &[&str]) -> Result<RunSpec, String> {
    let mut rest = Vec::new();
    let mut topology = std::collections::BTreeMap::new();
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        if matches!(*flag, "--panes" | "--clients" | "--load" | "--renderer") {
            let value = it.next().ok_or_else(|| format!("{flag} needs a value"))?;
            if topology.insert(*flag, *value).is_some() {
                return Err(format!("{flag} given twice"));
            }
        } else {
            rest.push(*flag);
        }
    }
    let get = |k: &str| topology.get(k).copied().ok_or(format!("{k} is required"));
    let panes = match get("--panes")? {
        "1" => 1,
        "15" => 15,
        other => return Err(format!("--panes must be 1|15, got {other}")),
    };
    let clients = match get("--clients")? {
        "1" => 1,
        "2" => 2,
        other => return Err(format!("--clients must be 1|2, got {other}")),
    };
    let load = match get("--load")? {
        "idle" => Load::Idle,
        "output" => Load::Output,
        other => return Err(format!("--load must be idle|output, got {other}")),
    };
    let renderer = match topology.get("--renderer") {
        None => RendererVariant::Control,
        Some(name) => RendererVariant::parse(name)?,
    };
    check_renderer_env(renderer.env())?;
    Ok(RunSpec {
        args: parse_args(&rest)?,
        panes,
        clients,
        load,
        renderer,
    })
}

/// Latency benchmark mode (spec 007 input → presentation), never mixed with resource modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LatencyMode {
    /// Calibration only (≤ 3 transitions, no acceptance claim).
    Smoke,
    /// Exactly 10 warmup + 100 measured transitions (110 raw attempts).
    Full,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LatencyArgs {
    pub mode: LatencyMode,
    pub warmup: usize,
    pub measured: usize,
    pub out: std::path::PathBuf,
}

pub const LATENCY_SMOKE_MAX_TRANSITIONS: usize = 3;

/// `--latency smoke --transitions N(1..=3) --out ABS` | `--latency full --out ABS`.
pub fn parse_latency_args(args: &[&str]) -> Result<LatencyArgs, String> {
    let mut values = std::collections::BTreeMap::new();
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        match *flag {
            "--latency" | "--transitions" | "--out" => {
                let value = it.next().ok_or_else(|| format!("{flag} needs a value"))?;
                if values.insert(*flag, *value).is_some() {
                    return Err(format!("{flag} given twice"));
                }
            }
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    let (mode, warmup, measured) = match (
        values.get("--latency").copied(),
        values.get("--transitions"),
    ) {
        (Some("smoke"), Some(n)) => {
            let n: usize = n.parse().map_err(|e| format!("--transitions: {e}"))?;
            if !(1..=LATENCY_SMOKE_MAX_TRANSITIONS).contains(&n) {
                return Err(format!(
                    "smoke runs 1..={LATENCY_SMOKE_MAX_TRANSITIONS} transitions, got {n}"
                ));
            }
            (LatencyMode::Smoke, 0, n)
        }
        (Some("smoke"), None) => return Err("smoke needs --transitions".into()),
        (Some("full"), None) => (LatencyMode::Full, 10, 100),
        (Some("full"), Some(_)) => {
            return Err("full runs exactly 10 + 100; --transitions refused".into())
        }
        (other, _) => return Err(format!("--latency must be smoke|full, got {other:?}")),
    };
    let out = std::path::PathBuf::from(values.get("--out").ok_or("--out is required")?);
    if !out.is_absolute() {
        return Err("--out must be absolute".into());
    }
    Ok(LatencyArgs {
        mode,
        warmup,
        measured,
        out,
    })
}
