#![cfg(loom)]

use std::sync::atomic::Ordering;

mod fixture;
use fixture::TrackNotify;
use fixture::Unit;

#[test]
fn new_and_delete() {
    loom::model(|| {
        let (tx, rx) = splitrc::new(Unit);
        drop(tx);
        drop(rx);
    })
}

#[test]
fn drop_rx_notifies() {
    loom::model(|| {
        let (tx, rx) = splitrc::new(TrackNotify::default());
        let rx2 = rx.clone();
        drop(rx);
        drop(rx2);
        assert!(!tx.tx_did_drop.load(Ordering::Acquire));
        assert!(tx.rx_did_drop.load(Ordering::Acquire));
    })
}

#[test]
fn drop_tx_notifies() {
    loom::model(|| {
        let (tx, rx) = splitrc::new(TrackNotify::default());
        let tx2 = tx.clone();
        drop(tx);
        drop(tx2);
        assert!(rx.tx_did_drop.load(Ordering::Acquire));
        assert!(!rx.rx_did_drop.load(Ordering::Acquire));
    })
}

#[test]
fn racing_drop() {
    loom::model(|| {
        let (tx, rx) = splitrc::new(TrackNotify::default());
        loom::thread::spawn(move || {
            _ = tx.rx_did_drop.load(Ordering::Acquire);
            drop(tx);
        });
        loom::thread::spawn(move || {
            _ = rx.tx_did_drop.load(Ordering::Acquire);
            drop(rx);
        });
    })
}

#[test]
#[ignore = "very slow"]
fn racing_drop_4_threads() {
    loom::model(|| {
        let (tx1, rx1) = splitrc::new(TrackNotify::default());
        let tx2 = tx1.clone();
        let rx2 = rx1.clone();
        loom::thread::spawn(move || {
            _ = tx1.rx_did_drop.load(Ordering::Acquire);
            drop(tx1);
        });
        loom::thread::spawn(move || {
            _ = tx2.rx_did_drop.load(Ordering::Acquire);
            drop(tx2);
        });
        loom::thread::spawn(move || {
            _ = rx1.tx_did_drop.load(Ordering::Acquire);
            drop(rx1);
        });
        loom::thread::spawn(move || {
            _ = rx2.tx_did_drop.load(Ordering::Acquire);
            drop(rx2);
        });
    })
}
