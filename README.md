# Bound

A minimal crate to encapsulate the act of deriving a struct from a reference. Notably useful
for wrapping `LockGuard` instances obtained from `Arc<RwLock<T>>` for example, but essentially
works with anything that has a similar relation as the LockGuard does to the RwLock.

## Usage

This and all other examples are also available on [https:://docs.rs/bound][docs]

```rs
use std::sync::{Arc, RwLock};
use bound::Bound;

let shared = Arc::new(RwLock::new(1));
let mut writer = Bound::try_new(shared.clone(), |a| a.write()).expect("Failed to lock");
*writer = 2;

// writer now has the following type
type Writer = Bound<RwLockWriteGuard<'static, usize>, Arc<RwLock<usize>>>
```

You can now safely pass `writer` around and put it in structs, something you couldn't do with an
`RwLockWriteGuard` by default because it's derived from the local reference to its lock.

[docs]: https://docs.rs/bound/0.2.1/bound/struct.Bound.html