# Thread-Safe Split Reference Counts

splitrc provides a pair of reference counts that together manage
the lifetime of an object. The object is notified when either
half's count reaches zero.

It is primarily aimed at implementing ownership of channel types,
where the channel needs notification when one half is fully
dropped.

The implementation stores two 32-bit counters in a shared 64-bit
atomic. On 64-bit platforms, atomic increment and decrements are
used. On 32-bit platforms, increments and decrements are a CAS
loop.

Four billion references should be plenty. Exceeding that leads to
a panic.

The pointers are arbitrarily named [Tx] and [Rx] to indicate their
intended use by channels.

```rust
# struct MyValue {}
# impl splitrc::Notify for MyValue {}
let (tx, rx) = splitrc::new(MyValue {});
```
