use std::fmt::{Debug, Display};
use std::future::IntoFuture;
use std::hash::Hash;
use std::mem;
use std::ops::{Deref, DerefMut};
use std::ptr;


/// Derive a T from a U and use it safely. U is placed on the heap, then
/// a reference to it is used to construct T which is stored inline.
/// This is not currently possible within Rust's type system so we need to use unsafe code.
/// 
/// # Usage
/// 
/// In abstract terms, the struct binds a lifetime like so:
/// 
/// ```
/// use bound::Bound;
/// // Struct with a lifetime parameter based on its reference field
/// struct Derived<'a>(&'a [usize]);
/// // Data that the struct must depend on
/// let data = vec![1, 2, 3];
/// let bound = Bound::new(data, |d| Derived(d.as_slice()));
/// // bound keeps alive the data and wraps the struct so that
/// // you just need to keep this struct in scope
/// assert_eq!(bound.0, &[1, 2, 3]);
/// ```
/// 
/// The canonical example is an `Arc<RwLock<>>`:
/// 
/// ```
/// use std::sync::{Arc, RwLock};
/// use bound::Bound;
/// let shared_data = Arc::new(RwLock::new(1));
/// let mut writer = Bound::try_new(shared_data.clone(), |a| a.write()).expect("Failed to lock");
/// **writer = 2;
/// ```
/// 
/// Normally you wouldn't be able to pass around a `RwLockWriteGuard`, but because this
/// is just a struct with no lifetime parameters, this works intuitively with a Bound.
pub struct Bound<G, U> {
    // Needed so that Drop can consume the T.
    // Safety: this will always be Some() until Drop is called
    derived: Option<G>,
    source_ptr: *mut U,
}

impl<'a, G, L: 'a> Bound<G, L> {
    /// Bind data to a referrent
    pub fn new<F: FnOnce(&'a mut L) -> G>(source: L, func: F) -> Self {
        // We need to create the box to avoid strange allocators
        let source_ref = Box::leak(Box::new(source));
        let source_ptr = source_ref as *mut L;
        let derived = func(source_ref);
        Self {
            derived: Some(derived),
            source_ptr
        }
    }

    /// Bind data to a referrent or a referrent error
    pub fn try_new<E, F>(source: L, func: F) -> Result<Self, Bound<E, L>>
    where F: FnOnce(&'a mut L) -> Result<G, E>{
        let source_ref = Box::leak(Box::new(source));
        let source_ptr = source_ref as *mut L;
        func(source_ref)
            .map(|res| Bound{ derived: Some(res), source_ptr })
            .map_err(|e| Bound{ derived: Some(e), source_ptr })
    }

    /// Bind data to an asynchronously obtained referrent
    pub async fn async_new<Fut, F>(source: L, func: F) -> Self
    where Fut: IntoFuture<Output = G>, F: FnOnce(&'a mut L) -> Fut {
        let source_ref = Box::leak(Box::new(source));
        let source_ptr = source_ref as *mut L;
        let derived = func(source_ref).await;
        Self {
            derived: Some(derived),
            source_ptr
        }
    }

    /// Bind data to an asynchronously obtained referrent or referrent error
    pub async fn async_try_new<E, Fut, F>(source: L, func: F) -> Result<Self, Bound<E, L>>
    where Fut: IntoFuture<Output = Result<G, E>>, F: FnOnce(&'a mut L) -> Fut {
        let source_ref = Box::leak(Box::new(source));
        let source_ptr = source_ref as *mut L;
        func(source_ref).await
            .map(|res| Bound{ derived: Some(res), source_ptr })
            .map_err(|e| Bound{ derived: Some(e), source_ptr })
    }

    /// Drop T and get back U
    pub fn unbind(mut self) -> Box<L> {
        mem::drop(self.derived.take());
        // Safety: a mutable reference to source was held by derived which was dropped just above
        // in addition, this function consumes self so it will only run once, and this is the
        // only place where source_ptr is set to null, so checking for null here is not necessary.
        let source = unsafe { Box::from_raw(self.source_ptr) };
        self.source_ptr = ptr::null_mut(); // Prevent Drop from deallocating source
        source
    }

    /// Access the wrapped struct
    pub fn wrapped(&self) -> &G {
        // Safety: this is always a Some() until Drop is called
        unsafe { self.derived.as_ref().unwrap_unchecked() }
    }

    /// Modify the wrapped struct. Making this public would be unsound because it might be
    /// overwritten
    fn wrapped_mut(&mut self) -> &mut G {
        // Safety: this is always a Some() until Drop is called
        unsafe { self.derived.as_mut().unwrap_unchecked() }
    }
}

/// Ensure that
/// - `derived` gets dropped before `source_ptr`
/// - `source_ptr` gets dropped
impl<G, L> Drop for Bound<G, L> {
    fn drop(&mut self) {
        mem::drop(self.derived.take());
        // Self::unbind might take the U out of the struct and set the pointer to null
        if !self.source_ptr.is_null() {
            // Safety: A mutable reference to source was held by derived, but derived was dropped
            // just above therefore the Box may safely be dropped.
            let source = unsafe { Box::from_raw(self.source_ptr) };
            mem::drop(source)
        };
    }
}

impl<T: ?Sized, G, U> Deref for Bound<G, U> where G: Deref<Target = T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        self.wrapped().deref()
    }
}

impl<T: ?Sized, G, U> DerefMut for Bound<G, U> where G: DerefMut<Target = T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.wrapped_mut().deref_mut()
    }
}

impl<T: ?Sized, G, L> AsRef<T> for Bound<G, L> where G: AsRef<T> {
    fn as_ref(&self) -> &T { self.wrapped().as_ref() }
}

impl<T: ?Sized, G, L> AsMut<T> for Bound<G, L> where G: DerefMut + AsMut<T> {
    fn as_mut(&mut self) -> &mut T { self.wrapped_mut().as_mut() }
}

#[cfg(test)]
mod tests {
    use std::ops::Deref;
    use std::sync::{Arc, RwLock, RwLockWriteGuard};

    use super::*;

    struct MockStore(&'static str);
    impl Drop for MockStore {
        fn drop(&mut self) {
            println!("Store {} dropped", self.0);
            self.0 = "FAIL";
        }
    }
    struct MockLock<'a>(&'a MockStore);
    impl Drop for MockLock<'_> {
        fn drop(&mut self) { println!("Lock for {} dropped", self.0.0) }
    }
    impl Deref for MockLock<'_> {
        type Target = str;
        fn deref(&self) -> &Self::Target {
            self.0.0
        }
    }

    fn mk_store() -> MockStore { MockStore("flag") }

    /// This is a compile-time error (but we can't test that)
    // fn mk_lock_naiive() -> MockLock<'static> { MockLock(&mk_store()) }

    /// Basic usage of Bound as a smart pointer
    fn mk_lock() -> Bound<MockLock<'static>, MockStore> {
        Bound::new(mk_store(), |s| MockLock(s))
    }

    #[test]
    fn test_functionality() {
        println!("-- testing basic functionality --");
        let bound = mk_lock();
        assert_eq!(bound.wrapped().0.0, "flag"); // can read wrapped (needed for error wrapping)
        assert_eq!(bound.deref(), "flag"); // can deref through wrapped if Deref
    }

    #[test]
    fn test_unbind() {
        println!("-- testing unbind --");
        let source = {
            let bound = mk_lock();
            bound.unbind()
        };
        println!("derived should be dropped but source is still alive");
        assert_eq!(source.0, "flag"); // source wasn't finalized with bound
    }

    /// This is extracted to demonstrate that the lock guard can have a static lifetime
    fn get_writer() -> Bound<RwLockWriteGuard<'static, usize>, Arc<RwLock<usize>>> {
        let shared_data = Arc::new(RwLock::new(1));
        Bound::new(shared_data.clone(), |a| a.write().unwrap())
    }

    #[test]
    fn test_rwlock() {
        println!("-- testing returning RwLockWriteGuard referencing Arc<RwLock> --");
        let mut writer = get_writer();
        *writer = 2;
    }
}

// Delegate std implementations to `self.deref()`

impl<G, L> Debug for Bound<G, L> where G: Debug {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Bound")
            .field(self.deref())
            .finish()
    }
}

impl<T: ?Sized, G, L> Display for Bound<G, L> where G: Deref<Target= T>, T: Display {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.deref().fmt(f)
    }
}

impl<T: ?Sized, G, L> Hash for Bound<G, L> where G: Deref<Target = T>, T: Hash {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) { self.deref().hash(state) }
}

impl<T: ?Sized, G, L> Eq for Bound<G, L> where G: Deref<Target = T>, T: Eq {}
impl<T: ?Sized, G, L, T2: ?Sized, G2, L2> PartialEq<Bound<G2, L2>> for Bound<G, L>
where G: Deref<Target = T>, G2: Deref<Target = T2>, T: PartialEq<T2> {
    fn eq(&self, other: &Bound<G2, L2>) -> bool { self.deref().eq(other.deref()) }
    fn ne(&self, other: &Bound<G2, L2>) -> bool { self.deref().ne(other.deref()) }
}

impl<T: ?Sized, G, L, T2: ?Sized, G2, L2> PartialOrd<Bound<G2, L2>> for Bound<G, L>
where G: Deref<Target = T>, G2: Deref<Target = T2>, T: PartialOrd<T2> {
    fn partial_cmp(&self, other: &Bound<G2, L2>) -> Option<std::cmp::Ordering> {
        self.deref().partial_cmp(&other.deref())
    }
}

impl<T: ?Sized, G, L> Ord for Bound<G, L> where G: Deref<Target = T>, T: Ord {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering { self.deref().cmp(other.deref()) }
}