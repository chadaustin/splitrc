use std::fmt;
use std::mem;
use std::panic;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::Arc;

#[derive(Debug)]
struct Unit;
impl splitrc::Notify for Unit {}

impl fmt::Display for Unit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt("Unit", f)
    }
}

#[derive(Default)]
struct TrackNotify {
    tx_did_drop: AtomicBool,
    rx_did_drop: AtomicBool,
}

impl splitrc::Notify for TrackNotify {
    fn last_tx_did_drop(&self) {
        self.tx_did_drop.store(true, Ordering::Release);
    }
    fn last_rx_did_drop(&self) {
        self.rx_did_drop.store(true, Ordering::Release);
    }
}

#[test]
fn new_and_delete() {
    let (tx, rx) = splitrc::new(Unit);
    drop(tx);
    drop(rx);
}

#[test]
fn drop_rx_notifies() {
    let (tx, rx) = splitrc::new(TrackNotify::default());
    let rx2 = rx.clone();
    drop(rx);
    drop(rx2);
    assert!(!tx.tx_did_drop.load(Ordering::Acquire));
    assert!(tx.rx_did_drop.load(Ordering::Acquire));
}

#[test]
fn drop_tx_notifies() {
    let (tx, rx) = splitrc::new(TrackNotify::default());
    let tx2 = tx.clone();
    drop(tx);
    drop(tx2);
    assert!(rx.tx_did_drop.load(Ordering::Acquire));
    assert!(!rx.rx_did_drop.load(Ordering::Acquire));
}

#[test]
fn debug_formatting() {
    assert_eq!("Unit", format!("{:?}", Unit));
    assert_eq!("Unit", format!("{:?}", Arc::new(Unit)));
    let (tx, rx) = splitrc::new(Unit);
    assert_eq!("Unit", format!("{:?}", tx));
    assert_eq!("Unit", format!("{:?}", rx));
}

#[test]
fn display_formatting() {
    assert_eq!("Unit", format!("{}", Unit));
    assert_eq!("Unit", format!("{}", Arc::new(Unit)));
    let (tx, rx) = splitrc::new(Unit);
    assert_eq!("Unit", format!("{}", tx));
    assert_eq!("Unit", format!("{}", rx));
}

#[test]
fn tx_panic_on_overflow() {
    let (tx, rx) = splitrc::new(Unit);
    drop(rx);

    let result = panic::catch_unwind(|| loop {
        mem::forget(tx.clone())
    });
    assert!(result.is_err());
}

#[test]
fn rx_panic_on_overflow() {
    let (tx, rx) = splitrc::new(Unit);
    drop(tx);

    let result = panic::catch_unwind(|| loop {
        mem::forget(rx.clone())
    });
    assert!(result.is_err());
}
