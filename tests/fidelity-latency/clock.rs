//! CLOCK_MONOTONIC read directly through the C library std already links (no new dependency).
//! `std::time::Instant` is not used: its epoch is not exposed, so it cannot be compared with
//! the ns stamped by the instrumented wtype or by Python `time.monotonic_ns()`.

#[repr(C)]
struct Timespec {
    tv_sec: i64,
    tv_nsec: i64,
}

extern "C" {
    fn clock_gettime(clock: i32, ts: *mut Timespec) -> i32;
    fn clock_getres(clock: i32, ts: *mut Timespec) -> i32;
}

/// Linux `CLOCK_MONOTONIC`.
pub const CLOCK_MONOTONIC: i32 = 1;
pub const CLOCK_NAME: &str = "CLOCK_MONOTONIC";

fn read(f: unsafe extern "C" fn(i32, *mut Timespec) -> i32) -> i64 {
    let mut ts = Timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `ts` is a valid, exclusively borrowed timespec (64-bit Linux layout).
    let rc = unsafe { f(CLOCK_MONOTONIC, &mut ts) };
    assert_eq!(rc, 0, "clock call failed");
    ts.tv_sec * 1_000_000_000 + ts.tv_nsec
}

pub fn monotonic_ns() -> i64 {
    read(clock_gettime)
}

pub fn monotonic_res_ns() -> i64 {
    read(clock_getres)
}
