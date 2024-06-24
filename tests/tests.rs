use std::marker::PhantomPinned;
use std::mem;
use std::panic;
use std::pin::Pin;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::sync::Barrier;

mod fixture;

use fixture::TrackNotify;
use fixture::Unit;

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
fn stress_threads() {
    const T: u64 = 16;
    let count = AtomicU64::new(0);
    let barrier = Arc::new(Barrier::new((T * 2) as usize));
    let barrier = &barrier;
    let (tx, rx) = splitrc::new(Count { count: &count });

    std::thread::scope(move |s| {
        for _ in 0..T {
            s.spawn({
                let tx = tx.clone();
                move || {
                    barrier.wait();
                    tx.count.fetch_add(1, Ordering::Relaxed);
                }
            });
            s.spawn({
                let rx = rx.clone();
                move || {
                    barrier.wait();
                    rx.count.fetch_add(1, Ordering::Relaxed);
                }
            });
        }
        drop(tx);
        drop(rx);
    });

    let final_count = count.load(Ordering::Relaxed);
    assert_eq!(1 + T + T, final_count);
}

#[test]
fn stress_dealloc() {
    const T: u64 = 64;
    let count = AtomicU64::new(0);

    std::thread::scope(|s| {
        for _ in 0..T {
            let (tx, rx) = splitrc::new(Count { count: &count });
            s.spawn(move || {
                tx.count.fetch_add(1, Ordering::Relaxed);
            });
            s.spawn(move || {
                rx.count.fetch_add(1, Ordering::Relaxed);
            });
        }
        for _ in 0..T {
            let (tx, rx) = splitrc::new(Count { count: &count });
            // Spawn in opposite order.
            s.spawn(move || {
                rx.count.fetch_add(1, Ordering::Relaxed);
            });
            s.spawn(move || {
                tx.count.fetch_add(1, Ordering::Relaxed);
            });
        }
    });

    let final_count = count.load(Ordering::Acquire);
    assert_eq!(6 * T, final_count);
}
