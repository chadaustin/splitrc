use std::fmt;
use std::sync::atomic::Ordering;

#[cfg(loom)]
use loom::sync::atomic::AtomicBool;

#[cfg(not(loom))]
use std::sync::atomic::AtomicBool;

#[derive(Debug)]
pub struct Unit;
impl splitrc::Notify for Unit {}

impl fmt::Display for Unit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt("Unit", f)
    }
}

#[derive(Default)]
pub struct TrackNotify {
    pub tx_did_drop: AtomicBool,
    pub rx_did_drop: AtomicBool,
}

impl splitrc::Notify for TrackNotify {
    fn last_tx_did_drop(&self) {
        self.tx_did_drop.store(true, Ordering::Release);
    }
    fn last_rx_did_drop(&self) {
        self.rx_did_drop.store(true, Ordering::Release);
    }
}
