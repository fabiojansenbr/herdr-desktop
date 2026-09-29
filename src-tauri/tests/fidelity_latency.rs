//! Latency measurement preparation of spec 007 (input → presentation, PRD p95 ≤ 50 ms /
//! p99 ≤ 100 ms). Proves the pure pieces only: cell marker encode/decode, raw record evaluator
//! (nearest-rank quantiles, conservative upper/lower bounds), the PTY helper, the clock shared
//! by the parent, the instrumented wtype header and Python. No display, no keys, no GUI:
//! nothing here measures latency. Needs host `python3` and `cc` only.
//! Linux-only: libc clock/PTY symbols and ioctl numbers are Linux 64-bit values.
#![cfg(all(target_os = "linux", target_pointer_width = "64"))]

#[path = "../../tests/fidelity-latency/mod.rs"]
mod latency;

use latency::clock::{monotonic_ns, monotonic_res_ns};
use latency::marker::{self, CellGrid, MarkerError, PageObservation, RgbView};
use latency::pty::{parse_helper_log, parse_wtype_log, HelperEvent};
use latency::record::{
    evaluate, nearest_rank, Attempt, Capture, Identity, Press, Run, Verdict, WtypeBuild, MEASURED,
    WARMUP,
};
use serde_json::json;
use std::path::PathBuf;

const MS: i64 = 1_000_000;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

// ------------------------------------------------------------------------------ marker

/// Paints the 48 marker cells of `seq` (optionally a different bit pattern) into an image
/// filled with a third colour, so an unpainted/misplaced sample is never a valid bit.
fn paint(bits: &[bool; 48], grid: &CellGrid, w: usize, h: usize, jitter: u8) -> Vec<u8> {
    let mut rgb = vec![0x40u8; w * h * 3];
    let y0 = (grid.origin_y + marker::ROW as f64 * grid.cell_h) as usize;
    let y1 = (grid.origin_y + (marker::ROW + 1) as f64 * grid.cell_h) as usize;
    for (i, bit) in bits.iter().enumerate() {
        let c = if *bit {
            marker::ON_RGB
        } else {
            marker::OFF_RGB
        };
        let c = [
            c[0].saturating_sub(jitter),
            c[1].saturating_add(jitter),
            c[2].saturating_sub(jitter),
        ];
        let x0 = (grid.origin_x + (marker::COL as f64 + i as f64) * grid.cell_w) as usize;
        let x1 = (grid.origin_x + (marker::COL as f64 + i as f64 + 1.0) * grid.cell_w) as usize;
        for y in y0..y1 {
            for x in x0..x1 {
                rgb[(y * w + x) * 3..(y * w + x) * 3 + 3].copy_from_slice(&c);
            }
        }
    }
    rgb
}

fn page() -> PageObservation {
    PageObservation {
        dpr: 1.25,
        canvas_left_css: 12.0,
        canvas_top_css: 40.0,
        cell_w_css: 8.4,
        cell_h_css: 17.0,
        cols: 75,
        rows: 29,
    }
}

fn grid() -> CellGrid {
    // CSD/window offset of the web view origin inside HEADLESS-1, measured by the parent.
    marker::grid(&page(), Some((31.0, 52.0))).unwrap()
}

#[test]
fn marker_bits_are_magic_seq_complement_suffix_msb_first() {
    let b = marker::bits(0x0102);
    let byte = |from: usize| (0..8).fold(0u8, |acc, i| (acc << 1) | b[from + i] as u8);
    // A little-endian or LSB-first encoder would give different bytes here.
    assert_eq!(
        [byte(0), byte(8), byte(16), byte(24), byte(32), byte(40)],
        [0xA5, 0x01, 0x02, 0xFE, 0xFD, 0x5A]
    );
}

#[test]
fn grid_requires_measured_offset_and_scales_css_by_dpr() {
    assert!(
        marker::grid(&page(), None).is_err(),
        "a zero CSD offset must never be assumed"
    );
    let g = grid();
    assert_eq!((g.origin_x, g.origin_y), (31.0 + 15.0, 52.0 + 50.0));
    assert_eq!((g.cell_w, g.cell_h), (10.5, 21.25));
    let narrow = PageObservation { cols: 51, ..page() };
    assert!(marker::grid(&narrow, Some((0.0, 0.0))).is_err());
    let short = PageObservation { rows: 4, ..page() };
    assert!(marker::grid(&short, Some((0.0, 0.0))).is_err());
    let bad_dpr = PageObservation { dpr: 0.0, ..page() };
    assert!(marker::grid(&bad_dpr, Some((0.0, 0.0))).is_err());
}

#[test]
fn page_observation_parses_the_ts_payload_keys() {
    let v = json!({"dpr": 1.25, "canvas_left_css": 12, "canvas_top_css": 40,
        "cell_w_css": 8.4, "cell_h_css": 17, "cols": 75, "rows": 29});
    assert_eq!(PageObservation::from_json(&v).unwrap(), page());
    let missing = json!({"dpr": 1.25, "canvas_left_css": 12, "cell_w_css": 8.4,
        "cell_h_css": 17, "cols": 75, "rows": 29});
    assert!(PageObservation::from_json(&missing).is_err());
}

#[test]
fn decode_reads_seq_within_tolerance_three() {
    let g = grid();
    let (w, h) = (700, 260);
    let img = paint(&marker::bits(0xBEEF), &g, w, h, 3);
    assert_eq!(
        marker::decode(
            RgbView {
                width: w,
                height: h,
                rgb: &img
            },
            &g
        ),
        Ok(0xBEEF)
    );
    let img = paint(&marker::bits(0xBEEF), &g, w, h, 4);
    assert!(matches!(
        marker::decode(
            RgbView {
                width: w,
                height: h,
                rgb: &img
            },
            &g
        ),
        Err(MarkerError::Ambiguous { .. })
    ));
}

#[test]
fn decode_rejects_malformed_masks() {
    let g = grid();
    let (w, h) = (700, 260);
    let decode = |bits: [bool; 48]| {
        let img = paint(&bits, &g, w, h, 0);
        marker::decode(
            RgbView {
                width: w,
                height: h,
                rgb: &img,
            },
            &g,
        )
    };
    let mut prefix = marker::bits(7);
    prefix[0] = !prefix[0];
    assert!(matches!(decode(prefix), Err(MarkerError::Prefix(0x25))));
    let mut complement = marker::bits(7);
    complement[39] = !complement[39];
    assert!(matches!(
        decode(complement),
        Err(MarkerError::Complement { .. })
    ));
    let mut suffix = marker::bits(7);
    suffix[47] = !suffix[47];
    assert!(matches!(decode(suffix), Err(MarkerError::Suffix(0x5B))));
    // Grid shifted by half a cell samples cell borders of the neighbour: must not decode 7.
    let img = paint(&marker::bits(7), &g, w, h, 0);
    let shifted = CellGrid {
        origin_x: g.origin_x + g.cell_w * 0.5 + 1.0,
        ..g
    };
    assert_ne!(
        marker::decode(
            RgbView {
                width: w,
                height: h,
                rgb: &img
            },
            &shifted
        ),
        Ok(7)
    );
    let tiny = RgbView {
        width: 100,
        height: 50,
        rgb: &img[..100 * 50 * 3],
    };
    assert!(matches!(
        marker::decode(tiny, &g),
        Err(MarkerError::OutOfBounds)
    ));
}

// ------------------------------------------------------------------------------ record

fn identity() -> Identity {
    Identity {
        host: "private-headless-7".into(),
        session: "hd-latency-disposable-7".into(),
        boot: "b0a7".into(),
        generation: 3,
        pane: "w1:p2".into(),
        dpr_milli: 1250,
        cols: 75,
        rows: 29,
    }
}

fn build() -> WtypeBuild {
    WtypeBuild {
        commit: "d71be3a7b3f93b534a2823fd68cabd7ac2a02359".into(),
        binary_sha256: "ef".repeat(32),
    }
}

/// Attempt `i` (0-based) at `t0`: stale capture then the new marker `upper` ns after press.
fn attempt(i: usize, t0: i64, upper: i64) -> Attempt {
    let seq = (i + 1) as u16;
    let press = t0 + 1_000;
    Attempt {
        index: i,
        expected_seq: seq,
        identity: identity(),
        press: Press {
            clock: "CLOCK_MONOTONIC".into(),
            index: 0,
            keycode: 38,
            ns: press,
        },
        released_ns: press + 2 * MS,
        helper: vec![HelperEvent {
            byte: b'a',
            count: (i + 1) as u64,
            seq,
        }],
        trusted_keydowns: 1,
        untrusted_keydowns: 0,
        captures: vec![
            Capture {
                clock: "CLOCK_MONOTONIC".into(),
                start_ns: press + MS / 2,
                end_ns: press + upper / 2,
                seq: Ok(seq - 1),
            },
            Capture {
                clock: "CLOCK_MONOTONIC".into(),
                start_ns: press + upper / 2 + 10,
                end_ns: press + upper,
                seq: Ok(seq),
            },
        ],
    }
}

fn run_with(uppers: &[i64]) -> Run {
    assert_eq!(uppers.len(), WARMUP + MEASURED);
    let mut t = 5_000 * MS;
    let attempts = uppers
        .iter()
        .enumerate()
        .map(|(i, u)| {
            let a = attempt(i, t, *u);
            t += u + 20 * MS;
            a
        })
        .collect();
    Run {
        clock: "CLOCK_MONOTONIC".into(),
        clock_res_ns: 1,
        identity: identity(),
        wtype: build(),
        expected_wtype: build(),
        attempts,
    }
}

/// Measured uppers 1..=100 ms, warmups huge (they must not count).
fn ramp() -> Vec<i64> {
    let mut v = vec![900 * MS; WARMUP];
    v.extend((1..=MEASURED as i64).map(|k| k * MS));
    v
}

fn invalid(run: &Run, needle: &str) {
    match evaluate(run).verdict {
        Verdict::Invalid(why) => assert!(
            why.iter().any(|w| w.contains(needle)),
            "expected {needle:?} in {why:?}"
        ),
        other => panic!("expected Invalid({needle}), got {other:?}"),
    }
}

#[test]
fn nearest_rank_uses_indexes_94_and_98_for_100_samples() {
    let sorted: Vec<i64> = (0..100).map(|i| i * 10).collect();
    assert_eq!(nearest_rank(&sorted, 95), Some(940));
    assert_eq!(nearest_rank(&sorted, 99), Some(980));
    assert_eq!(nearest_rank(&sorted[..3], 95), Some(20));
    assert_eq!(nearest_rank(&[], 95), None);
}

#[test]
fn quantile_boundaries_on_upper_bounds() {
    // Upper bounds shuffled; sorted ramp 1..=100 ms, so p95 = 95 ms: above target.
    let mut u = ramp();
    u[WARMUP..].reverse();
    let r = evaluate(&run_with(&u));
    assert_eq!(
        (r.p95_upper_ns, r.p99_upper_ns),
        (Some(95 * MS), Some(99 * MS))
    );
    assert_eq!(r.samples.len(), MEASURED);
    // Exactly at targets: 95 samples ≤ 50 ms, 4 at 100 ms, 1 far above → Met.
    let mut met = vec![900 * MS; WARMUP];
    met.extend(std::iter::repeat_n(50 * MS, 95));
    met.extend(std::iter::repeat_n(100 * MS, 4));
    met.push(1_500 * MS);
    assert_eq!(evaluate(&run_with(&met)).verdict, Verdict::Met);
    // One more sample over 50 ms moves p95 to 50 ms + 1 ns: no longer provable.
    let mut over = met.clone();
    over[WARMUP] = 50 * MS + 1;
    over[WARMUP + 1] = 100 * MS;
    let r = evaluate(&run_with(&over));
    assert_eq!(r.p95_upper_ns, Some(100 * MS));
    assert!(
        matches!(r.verdict, Verdict::Insufficient(_)),
        "{:?}",
        r.verdict
    );
    // p99 boundary: 99th value 100 ms + 1 → not Met.
    let mut p99 = met.clone();
    p99[WARMUP + 95] = 100 * MS + 1;
    p99[WARMUP + 96] = 100 * MS + 1;
    assert_ne!(evaluate(&run_with(&p99)).verdict, Verdict::Met);
}

#[test]
fn coarse_upper_bounds_are_insufficient_and_only_lower_bounds_prove_failure() {
    // Captures take 300 ms: upper bound says nothing; stale capture starts only 0.5 ms after
    // press, so no failure is proven either.
    let mut u = vec![900 * MS; WARMUP];
    u.extend(std::iter::repeat_n(300 * MS, MEASURED));
    let r = evaluate(&run_with(&u));
    assert!(
        matches!(r.verdict, Verdict::Insufficient(_)),
        "{:?}",
        r.verdict
    );
    // Stale captures that START late prove the marker was absent at that time.
    let mut run = run_with(&u);
    for a in run.attempts.iter_mut() {
        let press = a.press.ns;
        a.captures[0].start_ns = press + 120 * MS;
        a.captures[0].end_ns = press + 140 * MS;
    }
    let r = evaluate(&run);
    assert_eq!(r.p95_lower_ns, Some(120 * MS));
    assert_eq!(r.verdict, Verdict::NotMet);
}

#[test]
fn coarse_clock_resolution_is_insufficient() {
    let mut met = vec![900 * MS; WARMUP];
    met.extend(std::iter::repeat_n(10 * MS, MEASURED));
    let mut run = run_with(&met);
    assert_eq!(evaluate(&run).verdict, Verdict::Met);
    run.clock_res_ns = 4 * MS;
    assert!(matches!(evaluate(&run).verdict, Verdict::Insufficient(_)));
}

#[test]
fn stale_marker_only_is_a_timeout_not_a_sample() {
    let mut run = run_with(&ramp());
    let a = &mut run.attempts[WARMUP + 3];
    a.captures[1].seq = Ok(a.expected_seq - 1);
    invalid(&run, "timeout");
    // New marker found, but only after the 2 s budget.
    let mut run = run_with(&ramp());
    let a = &mut run.attempts[WARMUP + 3];
    a.captures[1].end_ns = a.press.ns + 2_000 * MS + 1;
    invalid(&run, "timeout");
}

#[test]
fn wrong_or_future_or_malformed_capture_is_invalid() {
    let mut run = run_with(&ramp());
    run.attempts[WARMUP].captures[0].seq = Ok(run.attempts[WARMUP].expected_seq + 1);
    invalid(&run, "wrong seq");
    let mut run = run_with(&ramp());
    run.attempts[WARMUP].captures[0].seq = Err("prefix 0x25".into());
    invalid(&run, "malformed");
    let mut run = run_with(&ramp());
    run.attempts[WARMUP + 1].expected_seq += 1;
    invalid(&run, "expected seq");
    // A capture after the first positive one means the loop did not stop at the first.
    let mut run = run_with(&ramp());
    let extra = run.attempts[WARMUP].captures[1].clone();
    run.attempts[WARMUP].captures.push(Capture {
        start_ns: extra.end_ns + 1,
        end_ns: extra.end_ns + 2,
        ..extra
    });
    invalid(&run, "after marker");
}

#[test]
fn mixed_clocks_negative_intervals_and_overlap_are_invalid() {
    let mut run = run_with(&ramp());
    run.attempts[WARMUP + 5].captures[1].clock = "CLOCK_REALTIME".into();
    invalid(&run, "clock");
    let mut run = run_with(&ramp());
    run.attempts[WARMUP + 5].press.clock = "CLOCK_BOOTTIME".into();
    invalid(&run, "clock");
    let mut run = run_with(&ramp());
    run.attempts[WARMUP + 5].captures[0].start_ns = run.attempts[WARMUP + 5].press.ns - 1;
    invalid(&run, "before press");
    let mut run = run_with(&ramp());
    let c = &mut run.attempts[WARMUP + 5].captures[0];
    c.end_ns = c.start_ns - 1;
    invalid(&run, "negative");
    // Next press before previous marker was captured: coalesced, not serial.
    let mut run = run_with(&ramp());
    let prev_end = run.attempts[WARMUP + 5].captures[1].end_ns;
    let next = &mut run.attempts[WARMUP + 6];
    let shift = next.press.ns - (prev_end - 1);
    next.press.ns -= shift;
    next.released_ns -= shift;
    for c in next.captures.iter_mut() {
        c.start_ns -= shift;
        c.end_ns -= shift;
    }
    invalid(&run, "serial");
}

#[test]
fn input_must_arrive_exactly_once_trusted_and_as_a() {
    type Mutation = Box<dyn Fn(&mut Attempt)>;
    let cases: Vec<(&str, Mutation)> = vec![
        ("helper", Box::new(|a| a.helper.clear())),
        ("helper", Box::new(|a| a.helper.push(a.helper[0].clone()))),
        ("helper", Box::new(|a| a.helper[0].byte = b'b')),
        ("helper", Box::new(|a| a.helper[0].count += 1)),
        ("helper", Box::new(|a| a.helper[0].seq += 1)),
        ("keydown", Box::new(|a| a.trusted_keydowns = 0)),
        ("keydown", Box::new(|a| a.trusted_keydowns = 2)),
        ("keydown", Box::new(|a| a.untrusted_keydowns = 1)),
        ("wtype", Box::new(|a| a.press.index = 1)),
    ];
    for (needle, mutate) in cases {
        let mut run = run_with(&ramp());
        mutate(&mut run.attempts[WARMUP + 7]);
        invalid(&run, needle);
    }
    let mut run = run_with(&ramp());
    run.attempts.pop();
    invalid(&run, "attempts");
}

#[test]
fn identity_and_build_must_match() {
    type Mutation = Box<dyn Fn(&mut Identity)>;
    let cases: Vec<Mutation> = vec![
        Box::new(|i| i.host = "private-headless-8".into()),
        // Same host/boot/generation/pane, different session.
        Box::new(|i| i.session = "hd-latency-disposable-8".into()),
        Box::new(|i| i.boot = "b0a8".into()),
        Box::new(|i| i.generation = 4),
        Box::new(|i| i.pane = "w1:p3".into()),
        Box::new(|i| i.dpr_milli = 1000),
        Box::new(|i| i.cols = 74),
    ];
    for mutate in cases {
        let mut run = run_with(&ramp());
        mutate(&mut run.attempts[WARMUP + 9].identity);
        invalid(&run, "identity");
    }
    let mut run = run_with(&ramp());
    run.identity.cols = 51;
    for a in run.attempts.iter_mut() {
        a.identity.cols = 51;
    }
    invalid(&run, "geometry");
    let mut run = run_with(&ramp());
    run.wtype.binary_sha256 = "00".repeat(32);
    invalid(&run, "wtype build");
}

// ------------------------------------------------------------------------------ logs

#[test]
fn wtype_log_parser_reads_press_lines_and_rejects_foreign_clock() {
    let text = "hd-latency v1 press index=0 keycode=38 clock=CLOCK_MONOTONIC res_ns=1 ns=123456789\n\
                hd-latency v1 released index=0 keycode=38 clock=CLOCK_MONOTONIC res_ns=1 ns=125456789\n";
    let log = parse_wtype_log(text).unwrap();
    assert_eq!(
        log.press,
        vec![Press {
            clock: "CLOCK_MONOTONIC".into(),
            index: 0,
            keycode: 38,
            ns: 123_456_789
        }]
    );
    assert_eq!(log.released_ns, vec![125_456_789]);
    assert_eq!(log.res_ns, 1);
    assert!(parse_wtype_log(&text.replace("MONOTONIC", "REALTIME")).is_err());
    assert!(parse_wtype_log("hd-latency v1 press index=0 keycode=38\n").is_err());
    assert!(parse_wtype_log(&text.replace("ns=123456789", "ns=-5")).is_err());
}

#[test]
fn helper_log_parser_keeps_every_byte_including_unexpected() {
    let text = "hd-pty v1 ready cols=75 rows=29 seq=0 ns=10\n\
                hd-pty v1 recv byte=0x61 count=1 seq=1 emitted=1 ns=20\n\
                hd-pty v1 recv byte=0x62 count=1 seq=1 emitted=0 ns=30\n";
    let events = parse_helper_log(text).unwrap();
    assert_eq!(
        events,
        vec![
            HelperEvent {
                byte: b'a',
                count: 1,
                seq: 1
            },
            HelperEvent {
                byte: b'b',
                count: 1,
                seq: 1
            },
        ]
    );
    assert!(parse_helper_log("hd-pty v1 recv byte=0x61 count=1\n").is_err());
}

// ------------------------------------------------------------------------------ clocks

#[test]
fn parent_clock_is_the_same_monotonic_clock_as_python() {
    let res = monotonic_res_ns();
    assert!(res > 0 && res <= 1_000, "resolution {res} ns");
    let before = monotonic_ns();
    let out = std::process::Command::new("python3")
        .args(["-c", "import time; print(time.monotonic_ns())"])
        .output()
        .expect("python3");
    let after = monotonic_ns();
    let py: i64 = String::from_utf8(out.stdout)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    // CLOCK_BOOTTIME/REALTIME or a different epoch (e.g. Instant arithmetic) falls outside.
    assert!(before <= py && py <= after, "{before} <= {py} <= {after}");
}

fn compile_probe(dir: &std::path::Path) -> PathBuf {
    let src = dir.join("probe.c");
    std::fs::write(
        &src,
        "#include \"hd_latency.h\"\nint main(void){int64_t t=hd_latency_now_ns();\
         hd_latency_line(\"press\",0,38,t);printf(\"%lld\\n\",(long long)t);\
         return hd_latency_fd()<0?3:0;}\n",
    )
    .unwrap();
    let bin = dir.join("probe");
    let status = std::process::Command::new("cc")
        .arg("-I")
        .arg(repo().join("tests/fidelity-latency/wtype"))
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("cc");
    assert!(status.success());
    bin
}

#[test]
fn wtype_instrumentation_header_uses_parent_clock_and_owned_log() {
    let dir = tempfile::tempdir().unwrap();
    let bin = compile_probe(dir.path());
    let log = dir.path().join("wtype.log");
    let before = monotonic_ns();
    let out = std::process::Command::new(&bin)
        .env("HD_LATENCY_LOG", &log)
        .output()
        .unwrap();
    let after = monotonic_ns();
    assert_eq!(out.status.code(), Some(0));
    let printed: i64 = String::from_utf8(out.stdout)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(before <= printed && printed <= after);
    let parsed = parse_wtype_log(&std::fs::read_to_string(&log).unwrap()).unwrap();
    assert_eq!(parsed.press[0].ns, printed);
    assert_eq!(parsed.res_ns, monotonic_res_ns());
    // Existing file is never appended to or truncated.
    let again = std::process::Command::new(&bin)
        .env("HD_LATENCY_LOG", &log)
        .output()
        .unwrap();
    assert_eq!(again.status.code(), Some(70));
    assert_eq!(std::fs::read_to_string(&log).unwrap().lines().count(), 1);
    // Disabled: no log written anywhere.
    let off = std::process::Command::new(&bin)
        .env_remove("HD_LATENCY_LOG")
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert_eq!(off.status.code(), Some(3));
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 3);
}

// ------------------------------------------------------------------------------ PTY helper

#[test]
fn pty_helper_emits_exact_marker_once_per_a_without_echo() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("helper.log");
    let mut pty = latency::pty::Pty::open(75, 29).unwrap();
    let mut child = pty
        .spawn(
            std::process::Command::new("python3")
                .arg(repo().join("tests/fidelity-latency/pty_marker.py"))
                .arg("--log")
                .arg(&log),
        )
        .unwrap();
    let initial = [b"\x1b[?25l".as_slice(), &marker::ansi(0)].concat();
    assert_eq!(
        pty.read_exact_timeout(initial.len(), 5_000).unwrap(),
        initial
    );
    pty.write(b"a").unwrap();
    assert_eq!(
        pty.read_exact_timeout(marker::ansi(1).len(), 5_000)
            .unwrap(),
        marker::ansi(1)
    );
    // Unexpected byte: logged, no marker, no echo, no advance.
    pty.write(b"b").unwrap();
    pty.write(b"a").unwrap();
    assert_eq!(
        pty.read_exact_timeout(marker::ansi(2).len(), 5_000)
            .unwrap(),
        marker::ansi(2)
    );
    assert!(
        pty.read_exact_timeout(1, 300).is_err(),
        "no extra output (echo/timer)"
    );
    child.kill().ok();
    child.wait().ok();
    let events = parse_helper_log(&std::fs::read_to_string(&log).unwrap()).unwrap();
    assert_eq!(
        events,
        vec![
            HelperEvent {
                byte: b'a',
                count: 1,
                seq: 1
            },
            HelperEvent {
                byte: b'b',
                count: 1,
                seq: 1
            },
            HelperEvent {
                byte: b'a',
                count: 2,
                seq: 2
            },
        ]
    );
}

#[test]
fn pty_helper_refuses_geometry_that_cannot_hold_the_marker() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("helper.log");
    let mut pty = latency::pty::Pty::open(51, 29).unwrap();
    let mut child = pty
        .spawn(
            std::process::Command::new("python3")
                .arg(repo().join("tests/fidelity-latency/pty_marker.py"))
                .arg("--log")
                .arg(&log),
        )
        .unwrap();
    assert_eq!(child.wait().unwrap().code(), Some(3));
    assert!(std::fs::read_to_string(&log)
        .unwrap()
        .contains("geometry cols=51 rows=29"));
}

// ------------------------------------------------------------------------------ viewport (live seam)

use latency::viewport::{
    confirm_offset, content_rect, focused_window_pid, locate_seq0, offset_from_client, ContentRect,
};

/// Two windows on one scale-1 output; only `pid` 4242 is the measured window.
fn tree(scale: f64, rect_y: i64) -> serde_json::Value {
    let window = |pid: u32, x: i64, y: i64| {
        json!({"type":"con","pid":pid,"rect":{"x":x,"y":y,"width":1280,"height":720},
               "window_rect":{"x":0,"y":0,"width":1280,"height":720},"nodes":[],"floating_nodes":[]})
    };
    json!({"type":"root","nodes":[{"type":"output","name":"HEADLESS-1","scale":scale,
        "nodes":[{"type":"workspace","nodes":[window(9999, 640, 0), window(4242, 0, rect_y)],"floating_nodes":[]}]}]})
}

#[test]
fn viewport_content_rect_is_the_measured_pid_window_not_another_or_scaled() {
    // Would catch: taking the first/focused window, or ignoring an output scale != 1.
    assert_eq!(
        content_rect(&tree(1.0, 7), 4242).unwrap(),
        ContentRect {
            x: 0.0,
            y: 7.0,
            width: 1280.0,
            height: 720.0
        }
    );
    assert!(content_rect(&tree(1.0, 7), 1).is_err(), "absent pid");
    assert!(content_rect(&tree(2.0, 7), 4242)
        .unwrap_err()
        .contains("scale"));
}

#[test]
fn viewport_offset_includes_the_csd_header_derived_from_client_height() {
    // Would catch: assuming zero offset or ignoring the GTK header above the web view.
    let c = ContentRect {
        x: 5.0,
        y: 7.0,
        width: 1280.0,
        height: 720.0,
    };
    assert_eq!(
        offset_from_client(c, 1280.0, 683.0, 1.0).unwrap(),
        (5.0, 44.0)
    );
    assert_eq!(
        offset_from_client(c, 1280.0, 720.0, 1.0).unwrap(),
        (5.0, 7.0)
    );
    assert!(
        offset_from_client(c, 1270.0, 683.0, 1.0).is_err(),
        "side decoration"
    );
    assert!(
        offset_from_client(c, 1280.0, 721.0, 1.0).is_err(),
        "client taller than window"
    );
}

#[test]
fn viewport_seq0_locator_finds_painted_marker_and_confirmation_is_within_1px() {
    let p = PageObservation {
        dpr: 1.0,
        cols: 75,
        rows: 29,
        ..page()
    };
    let actual = (3.0, 41.0);
    let g = marker::grid(&p, Some(actual)).unwrap();
    let (w, h) = (1280, 720);
    let rgb = paint(&marker::bits(0), &g, w, h, 2);
    let img = RgbView {
        width: w,
        height: h,
        rgb: &rgb,
    };
    // A prediction 4 px off still locates the painted edges; confirmation then rejects it.
    let located = locate_seq0(img, &p, (7.0, 37.0)).unwrap();
    assert!(
        (located.0 - actual.0).abs() <= 1.0 && (located.1 - actual.1).abs() <= 1.0,
        "{located:?}"
    );
    assert!(confirm_offset((7.0, 37.0), located).is_err());
    assert_eq!(confirm_offset(actual, located).unwrap(), actual);
    // seq 1 painted: not the seq-0 confirmation.
    let rgb1 = paint(&marker::bits(1), &g, w, h, 0);
    assert!(locate_seq0(
        RgbView {
            width: w,
            height: h,
            rgb: &rgb1
        },
        &p,
        actual
    )
    .is_err());
    // Nothing painted: refused, never a default offset.
    let blank = vec![0x40u8; w * h * 3];
    assert!(locate_seq0(
        RgbView {
            width: w,
            height: h,
            rgb: &blank
        },
        &p,
        actual
    )
    .is_err());
}

#[test]
fn viewport_focused_window_pid_is_the_focused_con_not_workspace_or_neighbour() {
    // Would catch: sending wtype while sway focus sits on the workspace or another pid.
    let window = |pid: u32, focused: bool| {
        json!({"type":"con","pid":pid,"focused":focused,"rect":{"x":0,"y":0,"width":1280,"height":720},
               "window_rect":{"x":0,"y":0,"width":1280,"height":720},"nodes":[],"floating_nodes":[]})
    };
    let tree_of = |workspace_focused: bool, windows: Vec<serde_json::Value>| {
        json!({"type":"root","focused":false,"nodes":[{"type":"output","name":"HEADLESS-1","focused":false,"scale":1.0,
            "nodes":[{"type":"workspace","focused":workspace_focused,"nodes":windows,"floating_nodes":[]}]}]})
    };
    assert_eq!(
        focused_window_pid(&tree_of(
            false,
            vec![window(9999, false), window(4242, true)]
        ))
        .unwrap(),
        4242
    );
    assert!(
        focused_window_pid(&tree_of(true, vec![window(4242, false)]))
            .unwrap_err()
            .contains("focused"),
        "workspace focused, no window"
    );
    assert!(
        focused_window_pid(&tree_of(
            false,
            vec![window(9999, true), window(4242, false)]
        ))
        .unwrap()
            != 4242,
        "neighbour focused"
    );
    assert!(focused_window_pid(&tree_of(false, vec![window(4242, false)])).is_err());
}
