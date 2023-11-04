use std::marker::PhantomData;
use std::ops::Deref;
use std::process::abort;
use std::ptr::NonNull;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

/// Allows the reference-counted object to know when the write end or
/// read end is dropped.
///
/// Exactly one of these functions will be called.
pub trait Notify {
    /// Called when the last [Tx] is dropped.
    ///
    /// WARNING: This function is called during a [Drop::drop]
    /// implementation. To avoid deadlock, ensure that it does not
    /// acquire a lock that may be held during unwinding.
    ///
    /// NOTE: Only called if there are live [Rx] references.
    fn last_tx_did_drop(&self) {}

    /// Called when the last [Rx] is dropped.
    ///
    /// WARNING: This function is called during a [Drop::drop]
    /// implementation. To avoid deadlock, ensure that it does not
    /// acquire a lock that may be held during unwinding.
    ///
    /// NOTE: Only called if there are live [Tx] references.
    fn last_rx_did_drop(&self) {}
}

// Two 32-bit reference counts are encoded in a single atomic 64-bit.
// 32 bits are enough for reasonable use. That is, four billion
// incoming references to a single object is likely an accident.
//
// Rust compiles AtomicU64 operations to a CAS loop on 32-bit ARM and
// x86. That's acceptable.

const TX_INC: u64 = 1 << 32;
const RX_INC: u64 = 1;
const RC_INIT: u64 = TX_INC + RX_INC;

// To avoid accidental overflow (mem::forget or a 4-billion entry
// Vec), which would lead to a user-after-free, we must detect
// overflow. A runaway ramp allows detecting overflow early enough to
// panic. If the runaway ramp is not long enough (that is, if tens of
// thousands of threads simultaneously increment the reference count),
// and the count reaches 2**32-1, then the process must abort.
const RUNAWAY_RAMP: u32 = u32::MAX - (1 << 16);
const RUNAWAY_MAX: u32 = u32::MAX;

fn tx_count(c: u64) -> u32 {
    (c >> 32) as _
}

fn rx_count(c: u64) -> u32 {
    c as _
}

struct Inner<T> {
    count: AtomicU64,
    data: T,
}

fn deallocate<T>(ptr: &NonNull<Inner<T>>) {
    // We brought the reference count to zero, so deallocate.
    drop(unsafe { Box::from_raw(ptr.as_ptr()) });
}

#[derive(Debug)]
pub struct Tx<T: Notify> {
    ptr: NonNull<Inner<T>>,
    phantom: PhantomData<T>,
}

unsafe impl<T: Sync + Send + Notify> Send for Tx<T> {}
unsafe impl<T: Sync + Send + Notify> Sync for Tx<T> {}

impl<T: Notify> Drop for Tx<T> {
    fn drop(&mut self) {
        let inner = unsafe { self.ptr.as_ref() };
        let old = inner.count.fetch_sub(TX_INC, Ordering::AcqRel);
        if tx_count(old) != 1 {
            return;
        }
        if rx_count(old) == 0 {
            deallocate(&self.ptr)
        } else {
            inner.data.last_tx_did_drop()
        }
    }
}

impl<T: Notify> Clone for Tx<T> {
    fn clone(&self) -> Self {
        let inner = unsafe { self.ptr.as_ref() };
        let old = inner.count.fetch_add(TX_INC, Ordering::Relaxed);
        if tx_count(old) < RUNAWAY_RAMP {
            return Tx { ..*self };
        }
        // very cold:
        if tx_count(old) >= RUNAWAY_MAX {
            abort()
        } else {
            inner.count.fetch_sub(TX_INC, Ordering::Relaxed);
            panic!("tx count overflow")
        }
    }
}

impl<T: Notify> Deref for Tx<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &unsafe { self.ptr.as_ref() }.data
    }
}

#[derive(Debug)]
pub struct Rx<T: Notify> {
    ptr: NonNull<Inner<T>>,
    phantom: PhantomData<T>,
}

unsafe impl<T: Sync + Send + Notify> Send for Rx<T> {}
unsafe impl<T: Sync + Send + Notify> Sync for Rx<T> {}

impl<T: Notify> Drop for Rx<T> {
    fn drop(&mut self) {
        let inner = unsafe { self.ptr.as_ref() };
        let old = inner.count.fetch_sub(RX_INC, Ordering::AcqRel);
        if rx_count(old) != 1 {
            return;
        }
        if tx_count(old) == 0 {
            deallocate(&self.ptr)
        } else {
            inner.data.last_rx_did_drop()
        }
    }
}

impl<T: Notify> Clone for Rx<T> {
    fn clone(&self) -> Self {
        let inner = unsafe { self.ptr.as_ref() };
        let old = inner.count.fetch_add(RX_INC, Ordering::Relaxed);
        if rx_count(old) < RUNAWAY_RAMP {
            return Rx { ..*self };
        }
        // very cold:
        if rx_count(old) >= RUNAWAY_MAX {
            abort()
        } else {
            inner.count.fetch_sub(RX_INC, Ordering::Relaxed);
            panic!("rx count overflow")
        }
    }
}

impl<T: Notify> Deref for Rx<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &unsafe { self.ptr.as_ref() }.data
    }
}

pub fn new<T: Notify>(data: T) -> (Tx<T>, Rx<T>) {
    let x = Box::new(Inner {
        count: AtomicU64::new(RC_INIT),
        data,
    });
    let r = Box::leak(x);
    (
        Tx {
            ptr: r.into(),
            phantom: PhantomData,
        },
        Rx {
            ptr: r.into(),
            phantom: PhantomData,
        },
    )
}

#[cfg(test)]
mod tests {
    use crate as splitrc;
    use std::sync::atomic::AtomicBool;
    use std::sync::atomic::Ordering;

    struct Unit;
    impl splitrc::Notify for Unit {}

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
}
