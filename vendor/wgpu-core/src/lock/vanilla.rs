//! Plain, uninstrumented wrappers around [`parking_lot`] lock types.
//!
//! These definitions are used when no particular lock instrumentation
//! Cargo feature is selected.
//!
//! NBE patch (2026-11-24): an opt-in hold-and-wait report, [`timing`]. It is
//! off unless `NBE_LOCK_TIMING=1`, so these stay uninstrumented in effect.

use core::{fmt, ops};

use crate::lock::rank::LockRank;

/// NBE patch (2026-11-24, the slow-GPU-poll investigation): who held a
/// wgpu-core lock while another thread waited on it. Not upstream.
///
/// Off unless `NBE_LOCK_TIMING=1`; when off, every acquire pays one cached
/// flag read. When on, each lock remembers its last acquirer (and an
/// [`RwLock`] its last reader). An acquire that waited at least
/// `NBE_LOCK_TIMING_MS` (default 10) logs, at warn under target `nbe_lock`:
/// - the lock's name;
/// - the waiting thread and how long it waited;
/// - the last acquirer before it, and how long that acquirer had held the
///   lock when the wait began.
///
/// The last acquirer is the thread that held the lock immediately before the
/// waiter got it.
#[cfg(feature = "std")]
mod timing {
    use core::sync::atomic::{AtomicU64, Ordering};
    use std::{string::String, sync::OnceLock, time::Instant, vec::Vec};

    pub(super) fn enabled() -> bool {
        static E: OnceLock<bool> = OnceLock::new();
        *E.get_or_init(|| std::env::var("NBE_LOCK_TIMING").as_deref() == Ok("1"))
    }

    fn threshold_ns() -> u64 {
        static T: OnceLock<u64> = OnceLock::new();
        *T.get_or_init(|| {
            std::env::var("NBE_LOCK_TIMING_MS")
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(10)
                * 1_000_000
        })
    }

    /// Nanoseconds since the first call, plus one: 0 means "never".
    pub(super) fn now_ns() -> u64 {
        static START: OnceLock<Instant> = OnceLock::new();
        START.get_or_init(Instant::now).elapsed().as_nanos() as u64 + 1
    }

    static NEXT: AtomicU64 = AtomicU64::new(1);
    static NAMES: std::sync::Mutex<Vec<(u64, String)>> = std::sync::Mutex::new(Vec::new());

    std::thread_local! {
        static ME: u64 = {
            let id = NEXT.fetch_add(1, Ordering::Relaxed);
            let name = std::thread::current().name().unwrap_or("<unnamed>").into();
            if let Ok(mut names) = NAMES.lock() {
                names.push((id, name));
            }
            id
        };
    }

    fn me() -> u64 {
        ME.with(|id| *id)
    }

    fn name_of(id: u64) -> String {
        if id == 0 {
            return "<none>".into();
        }
        NAMES
            .lock()
            .ok()
            .and_then(|names| {
                names
                    .iter()
                    .find(|(i, _)| *i == id)
                    .map(|(_, n)| n.clone())
            })
            .unwrap_or_else(|| std::format!("<thread {id}>"))
    }

    /// The last thread to take a lock, and when.
    pub(super) struct Holder {
        id: AtomicU64,
        since: AtomicU64,
    }

    impl Holder {
        pub(super) const fn new() -> Self {
            Holder {
                id: AtomicU64::new(0),
                since: AtomicU64::new(0),
            }
        }
        pub(super) fn get(&self) -> (u64, u64) {
            (
                self.id.load(Ordering::Relaxed),
                self.since.load(Ordering::Relaxed),
            )
        }
        pub(super) fn set(&self, at: u64) {
            self.id.store(me(), Ordering::Relaxed);
            self.since.store(at, Ordering::Relaxed);
        }
    }

    /// Log one wait if it reached the threshold. `before` is each
    /// (role, (holder id, holder since)) read before this thread took the lock.
    pub(super) fn report(
        lock: &'static str,
        kind: &str,
        start: u64,
        end: u64,
        before: &[(&str, (u64, u64))],
    ) {
        let waited = end.saturating_sub(start);
        if waited < threshold_ns() {
            return;
        }
        let ms = |ns: u64| ns as f64 / 1e6;
        let mut holders = String::new();
        for (role, (id, since)) in before {
            let held = if *since == 0 {
                0.0
            } else {
                ms(start.saturating_sub(*since))
            };
            holders.push_str(&std::format!(
                "; last {role} {} (held {held:.1} ms when the wait began)",
                name_of(*id)
            ));
        }
        log::warn!(
            target: "nbe_lock",
            "NBE-LOCK {lock} {kind}: {} waited {:.1} ms{holders}",
            name_of(me()),
            ms(waited),
        );
    }
}

#[cfg(not(feature = "std"))]
mod timing {
    pub(super) fn enabled() -> bool {
        false
    }
    pub(super) fn now_ns() -> u64 {
        0
    }
    pub(super) struct Holder;
    impl Holder {
        pub(super) const fn new() -> Self {
            Holder
        }
        pub(super) fn get(&self) -> (u64, u64) {
            (0, 0)
        }
        pub(super) fn set(&self, _at: u64) {}
    }
    pub(super) fn report(_: &'static str, _: &str, _: u64, _: u64, _: &[(&str, (u64, u64))]) {}
}

pub struct RankData;

/// A plain wrapper around [`parking_lot::Mutex`].
///
/// This is just like [`parking_lot::Mutex`], except that our [`new`]
/// method takes a rank, indicating where the new mutex should sit in
/// `wgpu-core`'s lock ordering. The rank names the lock in [`timing`]'s
/// reports and is otherwise ignored.
///
/// See the [`lock`] module documentation for other wrappers.
///
/// [`new`]: Mutex::new
/// [`lock`]: crate::lock
pub struct Mutex<T> {
    inner: parking_lot::Mutex<T>,
    name: &'static str,
    last: timing::Holder,
}

/// A guard produced by locking [`Mutex`].
///
/// This is just a wrapper around a [`parking_lot::MutexGuard`].
pub struct MutexGuard<'a, T>(parking_lot::MutexGuard<'a, T>);

impl<T> Mutex<T> {
    pub fn new(rank: LockRank, value: T) -> Mutex<T> {
        Mutex {
            inner: parking_lot::Mutex::new(value),
            name: rank.bit.member_name(),
            last: timing::Holder::new(),
        }
    }

    pub fn lock(&self) -> MutexGuard<'_, T> {
        if !timing::enabled() {
            return MutexGuard(self.inner.lock());
        }
        let start = timing::now_ns();
        let guard = self.inner.lock();
        let end = timing::now_ns();
        let before = self.last.get();
        self.last.set(end);
        timing::report(self.name, "lock", start, end, &[("holder", before)]);
        MutexGuard(guard)
    }

    pub fn into_inner(self) -> T {
        self.inner.into_inner()
    }
}

impl<'a, T> ops::Deref for MutexGuard<'a, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.0.deref()
    }
}

impl<'a, T> ops::DerefMut for MutexGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.0.deref_mut()
    }
}

impl<T: fmt::Debug> fmt::Debug for Mutex<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.inner.fmt(f)
    }
}

/// A plain wrapper around [`parking_lot::RwLock`].
///
/// This is just like [`parking_lot::RwLock`], except that our [`new`]
/// method takes a rank, indicating where the new mutex should sit in
/// `wgpu-core`'s lock ordering. The rank names the lock in [`timing`]'s
/// reports and is otherwise ignored.
///
/// See the [`lock`] module documentation for other wrappers.
///
/// [`new`]: RwLock::new
/// [`lock`]: crate::lock
pub struct RwLock<T> {
    inner: parking_lot::RwLock<T>,
    name: &'static str,
    last_writer: timing::Holder,
    last_reader: timing::Holder,
}

/// A read guard produced by locking [`RwLock`] as a reader.
///
/// This is just a wrapper around a [`parking_lot::RwLockReadGuard`].
pub struct RwLockReadGuard<'a, T>(parking_lot::RwLockReadGuard<'a, T>);

/// A write guard produced by locking [`RwLock`] as a writer.
///
/// This is just a wrapper around a [`parking_lot::RwLockWriteGuard`].
pub struct RwLockWriteGuard<'a, T>(parking_lot::RwLockWriteGuard<'a, T>);

impl<T> RwLock<T> {
    pub fn new(rank: LockRank, value: T) -> RwLock<T> {
        RwLock {
            inner: parking_lot::RwLock::new(value),
            name: rank.bit.member_name(),
            last_writer: timing::Holder::new(),
            last_reader: timing::Holder::new(),
        }
    }

    pub fn read(&self) -> RwLockReadGuard<'_, T> {
        if !timing::enabled() {
            return RwLockReadGuard(self.inner.read());
        }
        let start = timing::now_ns();
        let guard = self.inner.read();
        let end = timing::now_ns();
        let writer = self.last_writer.get();
        self.last_reader.set(end);
        timing::report(self.name, "read", start, end, &[("writer", writer)]);
        RwLockReadGuard(guard)
    }

    pub fn write(&self) -> RwLockWriteGuard<'_, T> {
        if !timing::enabled() {
            return RwLockWriteGuard(self.inner.write());
        }
        let start = timing::now_ns();
        let guard = self.inner.write();
        let end = timing::now_ns();
        let (writer, reader) = (self.last_writer.get(), self.last_reader.get());
        self.last_writer.set(end);
        timing::report(
            self.name,
            "write",
            start,
            end,
            &[("writer", writer), ("reader", reader)],
        );
        RwLockWriteGuard(guard)
    }

    /// Force an read-unlock operation on this lock.
    ///
    /// Safety:
    /// - A read lock must be held which is not held by a guard.
    pub unsafe fn force_unlock_read(&self, _data: RankData) {
        unsafe { self.inner.force_unlock_read() };
    }
}

impl<'a, T> RwLockReadGuard<'a, T> {
    // Forget the read guard, leaving the lock in a locked state with no guard.
    //
    // Equivalent to std::mem::forget, but preserves the information about the lock
    // rank.
    pub fn forget(this: Self) -> RankData {
        core::mem::forget(this.0);

        RankData
    }
}

impl<T: fmt::Debug> fmt::Debug for RwLock<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.inner.fmt(f)
    }
}

impl<'a, T> ops::Deref for RwLockReadGuard<'a, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.0.deref()
    }
}

impl<'a, T> ops::Deref for RwLockWriteGuard<'a, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.0.deref()
    }
}

impl<'a, T> ops::DerefMut for RwLockWriteGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.0.deref_mut()
    }
}
