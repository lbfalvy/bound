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

## 1.0?

1.0 means robust, verified and ready for production use. This package is as robust as I can make 
it, but it uses unsafe code, so I'd like at least 5 experienced Rust developers to approve of it.
If you're comfortable with unsafe code and you believe the code to be secure, please send me an
email.

As always, if you found a way to break it please open an issue with a reproducible example.

Authoritative Approvals: 1

[docs]: https://docs.rs/bound/0.2.1/bound/struct.Bound.html