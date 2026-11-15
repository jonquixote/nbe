//! The calling thread's own CPU time (R21's diagnostic, the user's ruling of
//! 2026-11-14).
//!
//! A render phase that takes 50 ms of wall time either did 50 ms of work or
//! spent most of it off the CPU (descheduled, or blocked on a lock). Wall time
//! cannot tell the two apart; the thread's CPU clock can. This reads
//! `CLOCK_THREAD_CPUTIME_ID`, which counts only the time the calling thread
//! itself ran.
//!
//! It lives here because the workspace denies `unsafe_code` everywhere but
//! this crate, and `clock_gettime` is an FFI call. It hands out a plain
//! `Duration`.

use std::time::Duration;

/// CPU time the calling thread has run so far. `Duration::ZERO` if the clock
/// cannot be read (it can on every macOS this engine supports).
pub fn thread_cpu_time() -> Duration {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `clock_gettime` writes one `timespec` through a valid pointer
    // to a local; CLOCK_THREAD_CPUTIME_ID needs no other argument.
    let rc = unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts) };
    if rc != 0 {
        return Duration::ZERO;
    }
    Duration::new(ts.tv_sec as u64, ts.tv_nsec as u32)
}

#[cfg(test)]
mod tests {
    use super::thread_cpu_time;
    use std::time::{Duration, Instant};

    /// Sleeping costs wall time and almost no CPU; spinning costs both. A
    /// clock that copied wall time would fail the first; one that read
    /// nothing would fail the second.
    #[test]
    fn the_thread_clock_counts_running_not_sleeping() {
        let c0 = thread_cpu_time();
        std::thread::sleep(Duration::from_millis(60));
        let slept = thread_cpu_time() - c0;
        assert!(
            slept < Duration::from_millis(20),
            "60 ms asleep counted {slept:?} of CPU"
        );

        let c1 = thread_cpu_time();
        let t1 = Instant::now();
        let mut x = 0u64;
        while t1.elapsed() < Duration::from_millis(60) {
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1);
        }
        std::hint::black_box(x);
        let spun = thread_cpu_time() - c1;
        assert!(
            spun > Duration::from_millis(20),
            "60 ms of spinning counted only {spun:?} of CPU"
        );
    }
}
