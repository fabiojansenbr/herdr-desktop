//! Raw measurement record and its evaluator. Each measured sample yields a conservative
//! UPPER bound (end of the first capture showing the new marker − ns stamped immediately before
//! the pressed virtual-keyboard request) and a LOWER bound (start of the last capture still
//! showing the previous marker − press). Upper bounds can prove the PRD targets; only lower
//! bounds can prove a miss; anything else is Insufficient. Structural defects are Invalid.

use super::clock::CLOCK_NAME;
use super::marker::{MIN_COLS, MIN_ROWS};
pub use super::pty::HelperEvent;

pub const WARMUP: usize = 10;
pub const MEASURED: usize = 100;
pub const TIMEOUT_NS: i64 = 2_000_000_000;
pub const P95_TARGET_NS: i64 = 50_000_000;
pub const P99_TARGET_NS: i64 = 100_000_000;
/// Coarser clocks cannot support ms-level bounds.
pub const MAX_CLOCK_RES_NS: i64 = 1_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub host: String,
    /// Herdr session name: host/boot/generation/pane alone do not qualify the endpoint.
    pub session: String,
    pub boot: String,
    pub generation: u64,
    pub pane: String,
    pub dpr_milli: u32,
    pub cols: u32,
    pub rows: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WtypeBuild {
    pub commit: String,
    pub binary_sha256: String,
}

/// One `hd-latency v1 press` line of the instrumented wtype.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Press {
    pub clock: String,
    /// type_keycode call index inside that wtype invocation; a fresh single-key run has 0.
    pub index: u32,
    pub keycode: u32,
    pub ns: i64,
}

/// One grim capture: `seq` is the decoded marker or the decode error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capture {
    pub clock: String,
    pub start_ns: i64,
    pub end_ns: i64,
    pub seq: Result<u16, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attempt {
    pub index: usize,
    pub expected_seq: u16,
    pub identity: Identity,
    pub press: Press,
    pub released_ns: i64,
    pub helper: Vec<HelperEvent>,
    pub trusted_keydowns: u32,
    pub untrusted_keydowns: u32,
    pub captures: Vec<Capture>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    pub clock: String,
    pub clock_res_ns: i64,
    pub identity: Identity,
    pub wtype: WtypeBuild,
    pub expected_wtype: WtypeBuild,
    pub attempts: Vec<Attempt>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sample {
    pub index: usize,
    pub upper_ns: i64,
    pub lower_ns: i64,
    pub captures: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Met,
    NotMet,
    Insufficient(Vec<String>),
    Invalid(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub samples: Vec<Sample>,
    pub p95_upper_ns: Option<i64>,
    pub p99_upper_ns: Option<i64>,
    pub p95_lower_ns: Option<i64>,
    pub p99_lower_ns: Option<i64>,
    pub verdict: Verdict,
}

/// Nearest rank on an ascending slice: rank = ceil(p·n/100), value at rank − 1.
pub fn nearest_rank(sorted: &[i64], percent: u32) -> Option<i64> {
    if sorted.is_empty() {
        return None;
    }
    let rank = (percent as usize * sorted.len()).div_ceil(100).max(1);
    sorted.get(rank - 1).copied()
}

fn check_attempt(
    run: &Run,
    i: usize,
    a: &Attempt,
    prev_end: Option<i64>,
    err: &mut Vec<String>,
) -> Option<Sample> {
    let n = err.len();
    let e = |m: String| format!("attempt {i}: {m}");
    if a.index != i || a.expected_seq as usize != i + 1 {
        err.push(e(format!(
            "index/expected seq {}/{}",
            a.index, a.expected_seq
        )));
    }
    if a.identity != run.identity {
        err.push(e(format!("identity {:?}", a.identity)));
    }
    if a.press.clock != CLOCK_NAME || a.captures.iter().any(|c| c.clock != CLOCK_NAME) {
        err.push(e("mixed clock".into()));
    }
    if a.press.index != 0 {
        err.push(e(format!(
            "wtype press index {} (not a fresh single-key run)",
            a.press.index
        )));
    }
    if a.released_ns < a.press.ns {
        err.push(e("wtype release before press".into()));
    }
    if let Some(prev) = prev_end {
        if a.press.ns <= prev {
            err.push(e(
                "press before previous marker capture ended (not serial)".into()
            ));
        }
    }
    let once = [HelperEvent {
        byte: b'a',
        count: i as u64 + 1,
        seq: a.expected_seq,
    }];
    if a.helper != once {
        err.push(e(format!("helper bytes {:?}", a.helper)));
    }
    if a.trusted_keydowns != 1 || a.untrusted_keydowns != 0 {
        err.push(e(format!(
            "keydown trusted={} untrusted={}",
            a.trusted_keydowns, a.untrusted_keydowns
        )));
    }
    let mut last_end = a.press.ns;
    let mut lower = 0;
    let mut found: Option<(i64, usize)> = None;
    for (k, c) in a.captures.iter().enumerate() {
        if c.start_ns < a.press.ns {
            err.push(e(format!("capture {k} before press")));
        }
        if c.end_ns < c.start_ns {
            err.push(e(format!("capture {k} negative interval")));
        }
        if k > 0 && c.start_ns < last_end {
            err.push(e(format!("capture {k} overlaps previous (not serial)")));
        }
        last_end = c.end_ns;
        if found.is_some() {
            err.push(e(format!("capture {k} after marker")));
            continue;
        }
        match &c.seq {
            Ok(s) if *s == a.expected_seq => found = Some((c.end_ns - a.press.ns, k + 1)),
            Ok(s) if *s == a.expected_seq.wrapping_sub(1) => lower = c.start_ns - a.press.ns,
            Ok(s) => err.push(e(format!("capture {k} wrong seq {s}"))),
            Err(m) => err.push(e(format!("capture {k} malformed marker: {m}"))),
        }
    }
    match found {
        Some((upper, captures)) if upper <= TIMEOUT_NS && err.len() == n => Some(Sample {
            index: i,
            upper_ns: upper,
            lower_ns: lower.max(0),
            captures,
        }),
        Some((upper, _)) if upper > TIMEOUT_NS => {
            err.push(e(format!("timeout: marker after {upper} ns")));
            None
        }
        None => {
            err.push(e("timeout: new marker never captured".into()));
            None
        }
        _ => None,
    }
}

pub fn evaluate(run: &Run) -> Report {
    let mut err = vec![];
    let mut insufficient = vec![];
    if run.clock != CLOCK_NAME {
        err.push(format!("run clock {}", run.clock));
    }
    if run.clock_res_ns <= 0 {
        err.push(format!("clock resolution {}", run.clock_res_ns));
    } else if run.clock_res_ns > MAX_CLOCK_RES_NS {
        insufficient.push(format!("clock resolution {} ns", run.clock_res_ns));
    }
    if run.wtype != run.expected_wtype || run.wtype.binary_sha256.len() != 64 {
        err.push(format!(
            "wtype build {:?} != {:?}",
            run.wtype, run.expected_wtype
        ));
    }
    if run.identity.cols < MIN_COLS || run.identity.rows < MIN_ROWS {
        err.push(format!(
            "geometry {}x{}",
            run.identity.cols, run.identity.rows
        ));
    }
    if run.attempts.len() != WARMUP + MEASURED {
        err.push(format!(
            "attempts {} != {}",
            run.attempts.len(),
            WARMUP + MEASURED
        ));
    }
    let mut samples = vec![];
    let mut prev_end = None;
    for (i, a) in run.attempts.iter().enumerate() {
        let s = check_attempt(run, i, a, prev_end, &mut err);
        prev_end = a.captures.last().map(|c| c.end_ns);
        if let Some(s) = s {
            if i >= WARMUP {
                samples.push(s);
            }
        }
    }
    let q = |f: fn(&Sample) -> i64, p| {
        let mut v: Vec<i64> = samples.iter().map(f).collect();
        v.sort_unstable();
        if v.len() == MEASURED {
            nearest_rank(&v, p)
        } else {
            None
        }
    };
    let (p95u, p99u) = (q(|s| s.upper_ns, 95), q(|s| s.upper_ns, 99));
    let (p95l, p99l) = (q(|s| s.lower_ns, 95), q(|s| s.lower_ns, 99));
    let verdict = if !err.is_empty() {
        Verdict::Invalid(err)
    } else if !insufficient.is_empty() {
        Verdict::Insufficient(insufficient)
    } else if p95u.is_some_and(|v| v <= P95_TARGET_NS) && p99u.is_some_and(|v| v <= P99_TARGET_NS) {
        Verdict::Met
    } else if p95l.is_some_and(|v| v > P95_TARGET_NS) || p99l.is_some_and(|v| v > P99_TARGET_NS) {
        Verdict::NotMet
    } else {
        Verdict::Insufficient(vec![format!(
            "upper p95={p95u:?} p99={p99u:?} lower p95={p95l:?} p99={p99l:?}"
        )])
    };
    Report {
        samples,
        p95_upper_ns: p95u,
        p99_upper_ns: p99u,
        p95_lower_ns: p95l,
        p99_lower_ns: p99l,
        verdict,
    }
}
