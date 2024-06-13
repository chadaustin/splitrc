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
        loom::thread::spawn(move || tx.access());
        loom::thread::spawn(move || rx.access());
    })
}

#[test]
fn racing_drop_two_tx() {
    loom::model(|| {
        let (tx1, rx) = splitrc::new(TrackNotify::default());
        let tx2 = tx1.clone();
        drop(rx);
        loom::thread::spawn(move || tx1.access());
        loom::thread::spawn(move || tx2.access());
    })
}

#[test]
fn racing_drop_two_rx() {
    loom::model(|| {
        let (tx, rx1) = splitrc::new(TrackNotify::default());
        let rx2 = rx1.clone();
        drop(tx);
        loom::thread::spawn(move || rx1.access());
        loom::thread::spawn(move || rx2.access());
    })
}

#[test]
#[ignore = "very slow"]
fn racing_drop_4_threads() {
    loom::model(|| {
        let (tx1, rx1) = splitrc::new(TrackNotify::default());
        let tx2 = tx1.clone();
        let rx2 = rx1.clone();
        loom::thread::spawn(move || tx1.access());
        loom::thread::spawn(move || tx2.access());
        loom::thread::spawn(move || rx1.access());
        loom::thread::spawn(move || rx2.access());
    })
}
