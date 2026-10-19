use std::sync::atomic::{AtomicBool, Ordering};

static OFFLINE: AtomicBool = AtomicBool::new(false);

pub fn enabled() -> bool {
    OFFLINE.load(Ordering::Relaxed)
}

pub fn set(enabled: bool) {
    OFFLINE.store(enabled, Ordering::Relaxed);
}
