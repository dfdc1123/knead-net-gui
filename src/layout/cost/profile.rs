//! 性能剖析 (profiling) 钩子: `--cfg profile_cost` 时启用的 AtomicU64 计数器。
//!
//! 默认 `cargo build` 不开, 跑 `RUSTFLAGS='--cfg profile_cost' cargo build --release` 才生效。
//! 主程序里调 `reset_cost_profile()` 在 SA 入口, `dump_cost_profile("prefix")` 在出口。

#[cfg(profile_cost)]
mod cost_profile {
    use std::sync::atomic::AtomicU64;
    pub static COLLECT: AtomicU64 = AtomicU64::new(0);
    pub static OOB: AtomicU64 = AtomicU64::new(0);
    pub static PIN: AtomicU64 = AtomicU64::new(0);
    pub static BBOX: AtomicU64 = AtomicU64::new(0);
    pub static MST: AtomicU64 = AtomicU64::new(0);
    pub static RAIL: AtomicU64 = AtomicU64::new(0);
    pub static COMPACT: AtomicU64 = AtomicU64::new(0);
    pub static CALLS: AtomicU64 = AtomicU64::new(0);
    pub static PRINTED: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);
}

#[cfg(profile_cost)]
#[macro_export] macro_rules! cp_collect {
    ($n:expr) => {
        $crate::layout::cost::cost_profile::COLLECT
            .fetch_add($n, std::sync::atomic::Ordering::Relaxed);
    };
}
#[cfg(profile_cost)]
#[macro_export] macro_rules! cp_pin {
    ($n:expr) => {
        $crate::layout::cost::cost_profile::PIN.fetch_add($n, std::sync::atomic::Ordering::Relaxed);
    };
}
#[cfg(profile_cost)]
#[macro_export] macro_rules! cp_bbox {
    ($n:expr) => {
        $crate::layout::cost::cost_profile::BBOX
            .fetch_add($n, std::sync::atomic::Ordering::Relaxed);
    };
}
#[cfg(profile_cost)]
#[macro_export] macro_rules! cp_mst {
    ($n:expr) => {
        $crate::layout::cost::cost_profile::MST.fetch_add($n, std::sync::atomic::Ordering::Relaxed);
    };
}
#[cfg(profile_cost)]
#[macro_export] macro_rules! cp_rail {
    ($n:expr) => {
        $crate::layout::cost::cost_profile::RAIL
            .fetch_add($n, std::sync::atomic::Ordering::Relaxed);
    };
}
#[cfg(profile_cost)]
#[macro_export] macro_rules! cp_compact {
    ($n:expr) => {
        $crate::layout::cost::cost_profile::COMPACT
            .fetch_add($n, std::sync::atomic::Ordering::Relaxed);
    };
}
#[cfg(profile_cost)]
#[macro_export] macro_rules! cp_call {
    () => {
        $crate::layout::cost::cost_profile::CALLS
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    };
}

#[cfg(not(profile_cost))]
#[macro_export] macro_rules! cp_collect {
    ($n:expr) => {};
}
#[cfg(not(profile_cost))]
#[macro_export] macro_rules! cp_pin {
    ($n:expr) => {};
}
#[cfg(not(profile_cost))]
#[macro_export] macro_rules! cp_bbox {
    ($n:expr) => {};
}
#[cfg(not(profile_cost))]
#[macro_export] macro_rules! cp_mst {
    ($n:expr) => {};
}
#[cfg(not(profile_cost))]
#[macro_export] macro_rules! cp_rail {
    ($n:expr) => {};
}
#[cfg(not(profile_cost))]
#[macro_export] macro_rules! cp_compact {
    ($n:expr) => {};
}
#[cfg(not(profile_cost))]
#[macro_export] macro_rules! cp_call {
    () => {};
}

#[cfg(profile_cost)]
pub fn dump_cost_profile(prefix: &str) {
    use std::sync::atomic::Ordering;
    let calls = cost_profile::CALLS.load(Ordering::Relaxed).max(1);
    eprintln!(
        "[costfast {prefix} sum ns] calls={} collect={} oob={} pin={} bbox={} mst={} rail={} compact={}",
        calls,
        cost_profile::COLLECT.load(Ordering::Relaxed),
        cost_profile::OOB.load(Ordering::Relaxed),
        cost_profile::PIN.load(Ordering::Relaxed),
        cost_profile::BBOX.load(Ordering::Relaxed),
        cost_profile::MST.load(Ordering::Relaxed),
        cost_profile::RAIL.load(Ordering::Relaxed),
        cost_profile::COMPACT.load(Ordering::Relaxed),
    );
}

#[cfg(profile_cost)]
pub fn reset_cost_profile() {
    use std::sync::atomic::Ordering;
    cost_profile::COLLECT.store(0, Ordering::Relaxed);
    cost_profile::OOB.store(0, Ordering::Relaxed);
    cost_profile::PIN.store(0, Ordering::Relaxed);
    cost_profile::BBOX.store(0, Ordering::Relaxed);
    cost_profile::MST.store(0, Ordering::Relaxed);
    cost_profile::RAIL.store(0, Ordering::Relaxed);
    cost_profile::COMPACT.store(0, Ordering::Relaxed);
    cost_profile::CALLS.store(0, Ordering::Relaxed);
    cost_profile::PRINTED.store(0, Ordering::Relaxed);
}

#[cfg(not(profile_cost))]
pub fn dump_cost_profile(_prefix: &str) {}
#[cfg(not(profile_cost))]
pub fn reset_cost_profile() {}

