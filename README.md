# Bound

A minimal crate to encapsulate the act of deriving a struct from a reference. Notably useful
for wrapping `LockGuard` instances obtained from `Arc<RwLock<T>>` for example, but essentially
works with anything that has a similar relation as the LockGuard does to the RwLock.