#![doc = include_str!("../README.md")]

use std::borrow::Borrow;
use std::fmt;
use std::marker::PhantomData;
use std::ops::Deref;
use std::pin::Pin;
use std::process::abort;
use std::ptr::NonNull;
use std::sync::atomic::Ordering;

#[cfg(loom)]
use loom::sync::atomic::AtomicU64;

#[cfg(not(loom))]
use std::sync::atomic::AtomicU64;

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

// Encoding, big-endian:
// * 31-bit tx count
// * 31-bit rx count
// * 2-bit drop count, dealloc == 2
//
// 31 bits is plenty for reasonable use. That is, two billion incoming
// references to a single object is likely an accident.
//
// The drop count allows concurrent notification on one half and drop
// on the other to avoid racing. The last half to finish will
// deallocate.
//
// Rust compiles AtomicU64 operations to a CAS loop on 32-bit ARM and
// x86. That's acceptable.

const TX_SHIFT: u8 = 33;
const RX_SHIFT: u8 = 2;
const DC_SHIFT: u8 = 0;

const TX_MASK: u32 = (1 << 31) - 1;
const RX_MASK: u32 = (1 << 31) - 1;
const DC_MASK: u8 = 3;

const TX_INC: u64 = 1 << TX_SHIFT;
const RX_INC: u64 = 1 << RX_SHIFT;
const DC_INC: u64 = 1 << DC_SHIFT;
const RC_INIT: u64 = TX_INC + RX_INC; // drop count = 0

fn tx_count(c: u64) -> u32 {
    (c >> TX_SHIFT) as u32 & TX_MASK
}

fn rx_count(c: u64) -> u32 {
    (c >> RX_SHIFT) as u32 & RX_MASK
}

fn drop_count(c: u64) -> u8 {
    (c >> DC_SHIFT) as u8 & DC_MASK
}

// To avoid accidental overflow (mem::forget or a 2-billion entry
// Vec), which would lead to a user-after-free, we must detect
// overflow. There are two ranges an overflow that stays within the
// panic range is allowed to undo the increment and panic. It's
// basically not possible, but if some freak scenario causes overflow
// into the abort zone, then the process is considered unrecoverable
// and the only option is abort.
//
// If the panic range could start at (1 << 31) then the hot path branch is
// a `js' instruction.
//
// Another approach is to increment with a CAS, and then we don't need
// ranges at all. But that might be more expensive. Are uncontended
// CAS on Apple Silicon and AMD Zen as fast as uncontended increment?
//
// Under contention, probably. [TODO: link]
const OVERFLOW_PANIC: u32 = 1 << 30;
const OVERFLOW_ABORT: u32 = u32::MAX - (1 << 16);

struct SplitCount(AtomicU64);

impl SplitCount {
    fn new() -> Self {
        Self(AtomicU64::new(RC_INIT))
    }

    fn inc_tx(&self) {
        // SAFETY: Increment always occurs from an existing reference,
        // and passing a reference to another thread is sufficiently
        // fenced, so relaxed is all that's necessary.
        let old = self.0.fetch_add(TX_INC, Ordering::Relaxed);
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
            self.0.fetch_sub(TX_INC, Ordering::Relaxed);
            panic!("tx count overflow")
        }
    }

    #[inline]
    fn dec_tx(&self) -> DecrementAction {
        let mut action = DecrementAction::Nothing;
        self.0
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |mut current| {
                current -= TX_INC;
                if tx_count(current) == 0 {
                    // We are the last tx reference. Should we drop or
                    // notify?
                    action = if rx_count(current) == 0 {
                        current += DC_INC;
                        if drop_count(current) == 1 {
                            // If drop_count was zero, the other half
                            // is notifying and will deallocate.
                            DecrementAction::Nothing
                        } else {
                            // If drop count was one, we are the last
                            // half.
                            DecrementAction::Drop
                        }
                    } else {
                        DecrementAction::Notify
                    }
                } else {
                    // We don't need to reset `action` because no
                    // conflicting update will increase tx_count
                    // again.
                }
                Some(current)
            })
            .unwrap();
        action
    }

    fn inc_rx(&self) {
        // SAFETY: Increment always occurs from an existing reference,
        // and passing a reference to another thread is sufficiently
        // fenced, so relaxed is all that's necessary.
        let old = self.0.fetch_add(RX_INC, Ordering::Relaxed);
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
            self.0.fetch_sub(RX_INC, Ordering::Relaxed);
            panic!("rx count overflow")
        }
    }

    #[inline]
    fn dec_rx(&self) -> DecrementAction {
        let mut action = DecrementAction::Nothing;
        self.0
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |mut current| {
                current -= RX_INC;
                if rx_count(current) == 0 {
                    // We are the last rx reference. Should we drop or
                    // notify?
                    action = if tx_count(current) == 0 {
                        // drop_count is either 0 or 1 here
                        current += DC_INC;
                        if drop_count(current) == 1 {
                            // If drop_count was zero, the other half
                            // is notifying and will deallocate.
                            DecrementAction::Nothing
                        } else {
                            // If drop count was one, we are the last
                            // half.
                            DecrementAction::Drop
                        }
                    } else {
                        DecrementAction::Notify
                    }
                } else {
                    // We don't need to reset `action` because no
                    // conflicting update will increase tx_count
                    // again.
                }
                Some(current)
            })
            .unwrap();
        action
    }

    /// Returns true if we should be deallocated.
    fn inc_drop_count(&self) -> bool {
        1 == self.0.fetch_add(DC_INC, Ordering::AcqRel)
    }
}

enum DecrementAction {
    Nothing,
    Notify,
    Drop,
}

struct Inner<T> {
    data: T,
    // Deref is more common than reference counting, so hint to the
    // compiler that the count should be stored at the end.
    count: SplitCount,
}

fn deallocate<T>(ptr: NonNull<Inner<T>>) {
    // drop(unsafe { Box::from_raw(ptr.as_ptr()) }); The following
    // should be equivalent to the above line, but for some reason I
    // don't understand, the following passes MIRI but the above
    // doesn't.

    // SAFETY: Reference count is zero. Deallocate and leave the pointer
    // dangling.
    unsafe {
        let ptr = ptr.as_ptr();
        std::ptr::drop_in_place(ptr);
        std::alloc::dealloc(ptr as *mut u8, std::alloc::Layout::new::<Inner<T>>());
    }
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
                unsafe { Pin::new_unchecked(&inner.data) }.last_tx_did_drop_pinned();
                if inner.count.inc_drop_count() {
                    deallocate(self.ptr);
                }
            }
            DecrementAction::Drop => {
                deallocate(self.ptr);
            }
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
                unsafe { Pin::new_unchecked(&inner.data) }.last_rx_did_drop_pinned();
                if inner.count.inc_drop_count() {
                    deallocate(self.ptr);
                }
            }
            DecrementAction::Drop => {
                deallocate(self.ptr);
            }
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
