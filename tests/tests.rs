use std::fmt;
use std::marker::PhantomPinned;
use std::mem;
use std::panic;
use std::pin::Pin;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU64;
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
#[ignore]
fn tx_panic_on_overflow() {
    let (tx, rx) = splitrc::new(Unit);
    drop(rx);

    let result = panic::catch_unwind(|| loop {
        mem::forget(tx.clone())
    });
    assert!(result.is_err());
}

#[test]
#[ignore]
fn rx_panic_on_overflow() {
    let (tx, rx) = splitrc::new(Unit);
    drop(tx);

    let result = panic::catch_unwind(|| loop {
        mem::forget(rx.clone())
    });
    assert!(result.is_err());
}

#[test]
fn pointers_are_unpinned() {
    let (tx, rx) = splitrc::new(Unit);
    let _: &dyn Unpin = &tx;
    let _: &dyn Unpin = &rx;
}

#[derive(Default)]
struct MustPin {
    v: u32,
    tx_did_drop: AtomicBool,
    rx_did_drop: AtomicBool,
    _pinned: PhantomPinned,
}

impl MustPin {
    fn drop_tx(self: Pin<&Self>) {
        self.tx_did_drop.store(true, Ordering::Release);
    }

    fn drop_rx(self: Pin<&Self>) {
        self.rx_did_drop.store(true, Ordering::Release);
    }
}

impl splitrc::Notify for MustPin {
    fn last_tx_did_drop_pinned(self: Pin<&Self>) {
        self.drop_tx()
    }
    fn last_rx_did_drop_pinned(self: Pin<&Self>) {
        self.drop_rx()
    }
}

#[test]
fn alloc_pinned() {
    let (tx, rx): (Pin<splitrc::Tx<MustPin>>, Pin<splitrc::Rx<MustPin>>) =
        splitrc::pin(Default::default());
    assert_eq!(0, tx.v);
    assert_eq!(0, rx.v);
}

#[test]
fn drop_tx_pinned() {
    let (tx, rx): (Pin<splitrc::Tx<MustPin>>, Pin<splitrc::Rx<MustPin>>) =
        splitrc::pin(Default::default());
    assert_eq!(false, tx.tx_did_drop.load(Ordering::Acquire));
    drop(tx);
    assert_eq!(true, rx.tx_did_drop.load(Ordering::Acquire));
}

#[test]
fn drop_rx_pinned() {
    let (tx, rx): (Pin<splitrc::Tx<MustPin>>, Pin<splitrc::Rx<MustPin>>) =
        splitrc::pin(Default::default());
    assert_eq!(false, rx.rx_did_drop.load(Ordering::Acquire));
    drop(rx);
    assert_eq!(true, tx.rx_did_drop.load(Ordering::Acquire));
}

struct Count<'a> {
    count: &'a AtomicU64,
}

impl splitrc::Notify for Count<'_> {
    fn last_tx_did_drop(&self) {
        self.count.fetch_add(1, Ordering::Relaxed);
    }

    fn last_rx_did_drop(&self) {
        self.count.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn stress() {
    const T: usize = 256;
    let count = AtomicU64::new(0);
    let (tx, rx) = splitrc::new(Count { count: &count });

    std::thread::scope(move |s| {
        for _ in 0..T {
            s.spawn({
                let tx = tx.clone();
                move || {
                    tx.count.fetch_add(1, Ordering::Relaxed);
                }
            });
            s.spawn({
                let rx = rx.clone();
                move || {
                    rx.count.fetch_add(1, Ordering::Relaxed);
                }
            });
        }
        drop(tx);
        drop(rx);
    });

    assert_eq!((1 + T + T) as u64, count.load(Ordering::Acquire));
}
