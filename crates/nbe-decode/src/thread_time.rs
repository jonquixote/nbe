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

/// The calling thread's user and system CPU time so far, from Mach's
/// `thread_basic_info` (microsecond resolution). The A/B of the user's ruling
/// of 2026-11-15 17:30 (log-only): a phase whose CPU is close to its wall
/// time may still be kernel work (page faults, for one) rather than the
/// engine's own; the system share says how much. Zeros off macOS, or if
/// the call fails.
pub fn thread_user_system() -> (Duration, Duration) {
    #[cfg(target_os = "macos")]
    {
        // SAFETY: `pthread_mach_thread_np(pthread_self())` names the calling
        // thread without taking a port reference (nothing to deallocate).
        // `thread_info` writes at most `count` words into `info`, a local of
        // exactly THREAD_BASIC_INFO_COUNT words, and updates `count`.
        unsafe {
            let port = libc::pthread_mach_thread_np(libc::pthread_self());
            let mut info: libc::thread_basic_info = std::mem::zeroed();
            let mut count = libc::THREAD_BASIC_INFO_COUNT;
            let rc = libc::thread_info(
                port,
                libc::THREAD_BASIC_INFO as libc::thread_flavor_t,
                &mut info as *mut libc::thread_basic_info as libc::thread_info_t,
                &mut count,
            );
            if rc != 0 {
                return (Duration::ZERO, Duration::ZERO);
            }
            let d = |t: libc::time_value_t| {
                Duration::from_secs(t.seconds.max(0) as u64)
                    + Duration::from_micros(t.microseconds.max(0) as u64)
            };
            (d(info.user_time), d(info.system_time))
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        (Duration::ZERO, Duration::ZERO)
    }
}

/// The whole process's page faults and context switches so far
/// (`getrusage(RUSAGE_SELF)`). Process-wide: macOS keeps no per-thread
/// counters for these, so they include every thread's (decode, audio,
/// upload), not only the render thread's.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProcessCounts {
    pub minor_faults: u64,
    pub major_faults: u64,
    pub involuntary_switches: u64,
    pub voluntary_switches: u64,
}

impl ProcessCounts {
    /// The counts since `earlier`.
    pub fn since(self, earlier: ProcessCounts) -> ProcessCounts {
        ProcessCounts {
            minor_faults: self.minor_faults.saturating_sub(earlier.minor_faults),
            major_faults: self.major_faults.saturating_sub(earlier.major_faults),
            involuntary_switches: self
                .involuntary_switches
                .saturating_sub(earlier.involuntary_switches),
            voluntary_switches: self
                .voluntary_switches
                .saturating_sub(earlier.voluntary_switches),
        }
    }
}

/// The process's counts so far; zeros if `getrusage` fails.
pub fn process_counts() -> ProcessCounts {
    // SAFETY: `getrusage` writes one `rusage` through a valid pointer to a
    // zeroed local; RUSAGE_SELF needs no other argument.
    let ru = unsafe {
        let mut ru: libc::rusage = std::mem::zeroed();
        if libc::getrusage(libc::RUSAGE_SELF, &mut ru) != 0 {
            return ProcessCounts::default();
        }
        ru
    };
    ProcessCounts {
        minor_faults: ru.ru_minflt.max(0) as u64,
        major_faults: ru.ru_majflt.max(0) as u64,
        involuntary_switches: ru.ru_nivcsw.max(0) as u64,
        voluntary_switches: ru.ru_nvcsw.max(0) as u64,
    }
}

#[cfg(test)]
mod tests {
    use super::{process_counts, thread_cpu_time, thread_user_system};
    use std::time::{Duration, Instant};

    /// Spinning is user time; touching fresh pages is minor faults on the
    /// process and system time on the thread that touched them. A reader that
    /// returned zeros, or copied one figure into another, fails one of these.
    #[test]
    fn user_time_system_time_and_faults_are_each_measured() {
        let (u0, s0) = thread_user_system();
        let t = Instant::now();
        let mut x = 0u64;
        while t.elapsed() < Duration::from_millis(80) {
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1);
        }
        std::hint::black_box(x);
        let (u1, s1) = thread_user_system();
        assert!(
            u1 - u0 > Duration::from_millis(30),
            "80 ms of spinning counted only {:?} of user time",
            u1 - u0
        );
        assert!(
            s1 - s0 < Duration::from_millis(30),
            "80 ms of spinning counted {:?} of system time",
            s1 - s0
        );

        // 256 MiB, one write per 4 KiB: every page is faulted in fresh.
        let c0 = process_counts();
        let (_, s2) = thread_user_system();
        let mut buf: Vec<u8> = vec![0; 256 << 20];
        for i in (0..buf.len()).step_by(4096) {
            buf[i] = 1;
        }
        std::hint::black_box(&buf);
        let (_, s3) = thread_user_system();
        let faults = process_counts().since(c0);
        drop(buf);
        assert!(
            faults.minor_faults > 1_000,
            "touching 256 MiB of fresh memory counted {faults:?}"
        );
        assert!(
            s3 - s2 > Duration::from_millis(1),
            "faulting in 256 MiB counted only {:?} of system time",
            s3 - s2
        );
    }

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
