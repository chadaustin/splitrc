#![doc = include_str!("../README.md")]

use std::borrow::Borrow;
use std::fmt;
use std::marker::PhantomData;
use std::ops::Deref;
use std::pin::Pin;
use std::process::abort;
use std::ptr::NonNull;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

#[cfg(doc)]
use std::marker::Unpin;

// TODO:
// * Missing trait implementations
// * Error
// * Pointer
// * Eq, PartialEq
// * Ord, PartialOrd
// * Hash

/// Allows the reference-counted object to know when the last write
/// reference or the last read reference is dropped.
///
/// Exactly one of these functions will be called.
pub trait Notify {
    /// Called when the last [Tx] is dropped. By default, delegates to
    /// [Notify::last_tx_did_drop].
    fn last_tx_did_drop_pinned(self: Pin<&Self>) {
        self.get_ref().last_tx_did_drop()
    }

    /// Called when the last [Tx] is dropped.
    ///
    /// WARNING: This function is called during a [Drop::drop]
    /// implementation. To avoid deadlock, ensure that it does not
    /// acquire a lock that may be held during unwinding.
    ///
    /// NOTE: Only called if there are live [Rx] references.
    fn last_tx_did_drop(&self) {}

    /// Called when the last [Rx] is dropped. By default, delegates to
    /// [Notify::last_rx_did_drop].
    fn last_rx_did_drop_pinned(self: Pin<&Self>) {
        self.get_ref().last_rx_did_drop()
    }

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
// overflow. There are two ranges an overflow that stays within the
// panic range is allowed to undo the increment and panic. It's
// basically not possible, but if some freak scenario causes overflow
// into the abort zone, then the process is considered unrecoverable
// and the only option is abort.
//
// If the panic range starts at (1 << 31) then the hot path branch is
// a `js' instruction.
const OVERFLOW_PANIC: u32 = 1 << 31;
const OVERFLOW_ABORT: u32 = u32::MAX - (1 << 16);

struct SplitCount {
    count: AtomicU64,
}

impl SplitCount {
    fn new() -> Self {
        Self {
            count: AtomicU64::new(RC_INIT),
        }
    }

    fn inc_tx(&self) {
        let old = self.count.fetch_add(TX_INC, Ordering::Relaxed);
        if tx_count(old) < OVERFLOW_PANIC {
            return;
        }
        self.inc_tx_overflow(old)
    }

    #[cold]
    fn inc_tx_overflow(&self, old: u64) {
        if tx_count(old) >= OVERFLOW_ABORT {
            abort()
        } else {
            self.count.fetch_sub(TX_INC, Ordering::Relaxed);
            panic!("tx count overflow")
        }
    }

    #[inline]
    fn dec_tx(&self) -> DecrementAction {
        let old = self.count.fetch_sub(TX_INC, Ordering::AcqRel);
        if tx_count(old) != 1 {
            DecrementAction::Nothing
        } else if rx_count(old) != 0 {
            DecrementAction::Notify
        } else {
            DecrementAction::Drop
        }
    }

    fn inc_rx(&self) {
        let old = self.count.fetch_add(RX_INC, Ordering::Relaxed);
        if rx_count(old) < OVERFLOW_PANIC {
            return;
        }
        self.inc_rx_overflow(old)
    }

    #[cold]
    fn inc_rx_overflow(&self, old: u64) {
        if rx_count(old) >= OVERFLOW_ABORT {
            abort()
        } else {
            self.count.fetch_sub(RX_INC, Ordering::Relaxed);
            panic!("rx count overflow")
        }
    }

    #[inline]
    fn dec_rx(&self) -> DecrementAction {
        let old = self.count.fetch_sub(RX_INC, Ordering::AcqRel);
        if rx_count(old) != 1 {
            DecrementAction::Nothing
        } else if tx_count(old) != 0 {
            DecrementAction::Notify
        } else {
            DecrementAction::Drop
        }
    }
}

enum DecrementAction {
    Nothing,
    Notify,
    Drop,
}

fn tx_count(c: u64) -> u32 {
    (c >> 32) as _
}

fn rx_count(c: u64) -> u32 {
    c as _
}

struct Inner<T> {
    data: T,
    // Deref is more common than reference counting, so hint to the
    // compiler that the count should be stored at the end.
    count: SplitCount,
}

fn deallocate<T>(ptr: &NonNull<Inner<T>>) {
    // SAFETY: Reference count is zero. Deallocate and leave it
    // dangling.
    drop(unsafe { Box::from_raw(ptr.as_ptr()) });
}

/// The write half of a split reference count.
pub struct Tx<T: Notify> {
    ptr: NonNull<Inner<T>>,
    phantom: PhantomData<T>,
}

unsafe impl<T: Sync + Send + Notify> Send for Tx<T> {}
unsafe impl<T: Sync + Send + Notify> Sync for Tx<T> {}

impl<T: Notify> Drop for Tx<T> {
    fn drop(&mut self) {
        // SAFETY: We do not create a &mut to Inner.
        let inner = unsafe { self.ptr.as_ref() };
        match inner.count.dec_tx() {
            DecrementAction::Nothing => (),
            DecrementAction::Notify => {
                // SAFETY: data is never moved
                unsafe { Pin::new_unchecked(&inner.data) }.last_tx_did_drop_pinned()
            }
            DecrementAction::Drop => deallocate(&self.ptr),
        }
    }
}

impl<T: Notify> Clone for Tx<T> {
    fn clone(&self) -> Self {
        // SAFETY: We do not create a &mut to Inner.
        let inner = unsafe { self.ptr.as_ref() };
        inner.count.inc_tx();
        Tx { ..*self }
    }
}

impl<T: Notify> Deref for Tx<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        // SAFETY: We know ptr is valid and do not create &mut.
        &unsafe { self.ptr.as_ref() }.data
    }
}

impl<T: Notify> AsRef<T> for Tx<T> {
    fn as_ref(&self) -> &T {
        self.deref()
    }
}

impl<T: Notify> Borrow<T> for Tx<T> {
    fn borrow(&self) -> &T {
        self.deref()
    }
}

impl<T: Notify + fmt::Debug> fmt::Debug for Tx<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_ref(), f)
    }
}

impl<T: Notify + fmt::Display> fmt::Display for Tx<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self.as_ref(), f)
    }
}

/// The read half of a split reference count.
pub struct Rx<T: Notify> {
    ptr: NonNull<Inner<T>>,
    phantom: PhantomData<T>,
}

unsafe impl<T: Sync + Send + Notify> Send for Rx<T> {}
unsafe impl<T: Sync + Send + Notify> Sync for Rx<T> {}

impl<T: Notify> Drop for Rx<T> {
    fn drop(&mut self) {
        // SAFETY: We do not create a &mut to Inner.
        let inner = unsafe { self.ptr.as_ref() };
        match inner.count.dec_rx() {
            DecrementAction::Nothing => (),
            DecrementAction::Notify => {
                // SAFETY: data is never moved
                unsafe { Pin::new_unchecked(&inner.data) }.last_rx_did_drop_pinned()
            }
            DecrementAction::Drop => deallocate(&self.ptr),
        }
    }
}

impl<T: Notify> Clone for Rx<T> {
    fn clone(&self) -> Self {
        // SAFETY: We do not create a &mut to Inner.
        let inner = unsafe { self.ptr.as_ref() };
        inner.count.inc_rx();
        Rx { ..*self }
    }
}

impl<T: Notify> Deref for Rx<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        // SAFETY: We know ptr is valid and do not create &mut.
        &unsafe { self.ptr.as_ref() }.data
    }
}

impl<T: Notify> AsRef<T> for Rx<T> {
    fn as_ref(&self) -> &T {
        self.deref()
    }
}

impl<T: Notify> Borrow<T> for Rx<T> {
    fn borrow(&self) -> &T {
        self.deref()
    }
}

impl<T: Notify + fmt::Debug> fmt::Debug for Rx<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_ref(), f)
    }
}

impl<T: Notify + fmt::Display> fmt::Display for Rx<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self.as_ref(), f)
    }
}

/// Allocates a pointer holding `data` and returns a pair of references.
///
/// T must implement [Notify] to receive a notification when the write
/// half or read half are dropped.
///
/// `data` is dropped when both halves' reference counts reach zero.
pub fn new<T: Notify>(data: T) -> (Tx<T>, Rx<T>) {
    let x = Box::new(Inner {
        count: SplitCount::new(),
        data,
    });
    // SAFETY: We just allocated the box, so it's not null.
    let ptr = unsafe { NonNull::new_unchecked(Box::into_raw(x)) };
    (
        Tx {
            ptr,
            phantom: PhantomData,
        },
        Rx {
            ptr,
            phantom: PhantomData,
        },
    )
}

/// Allocates a pointer holding `data` and returns a pair of pinned
/// references.
///
/// The rules are the same as [new] except that the memory is pinned
/// in place and cannot be moved again, unless `T` implements [Unpin].
pub fn pin<T: Notify>(data: T) -> (Pin<Tx<T>>, Pin<Rx<T>>) {
    let (tx, rx) = new(data);
    // SAFETY: data is never moved again
    unsafe { (Pin::new_unchecked(tx), Pin::new_unchecked(rx)) }
}
