use std::fmt::{Debug, Display};
use std::future::{IntoFuture, Future};
use std::hash::Hash;
use std::mem;
use std::ops::{Deref, DerefMut};
use std::ptr;


/// Derive a struct from a reference and use it safely. The source is boxed,
/// then a reference to it is used to construct the dependent data which is stored inline.
/// This is not currently possible within Rust's type system so we need to use unsafe code.
/// 
/// # Usage
/// 
/// In abstract terms, the struct binds a lifetime and a dependent like so:
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
/// assert_eq!(bound.wrapped().0, &[1, 2, 3]);
/// ```
/// 
/// If the dependent is something like a mutex guard you can write /// through it.
/// To be precise, [Deref], [DerefMut], [AsRef] and [AsMut] are all forwarded if
/// the dependent implements them.
/// 
/// The canonical example is an [Arc]<[RwLock]>:
/// 
/// ```
/// use std::sync::{Arc, RwLock};
/// use bound::Bound;
/// let shared_data = Arc::new(RwLock::new(1));
/// let mut writer = Bound::try_new(shared_data.clone(), |a| a.write()).expect("Failed to lock");
/// *writer = 2;
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

impl<G, L> Bound<G, L> {
    /// Take the parts out of a Box and leave it empty so Drop doesn't do anything
    /// # Safety
    /// Derived still holds a mutable reference to source_ptr so casting it back to box is unsound
    fn decompose(mut self) -> (G, *mut L) {
        let g = self.derived.take().unwrap();
        let source_ptr = self.source_ptr;
        self.source_ptr = ptr::null_mut(); // Prevent Drop from deallocating source
        (g, source_ptr)
    }

    /// Drop the referent and get back the source
    pub fn unbind(self) -> Box<L> {
        let (derived, source_ptr) = self.decompose();
        mem::drop(derived);
        // Safety: a mutable reference to source was held by derived which was dropped just above
        // in addition, this function consumes self so it will only run once, and this is the
        // only place where source_ptr is set to null, so checking for null here is not necessary.
        unsafe { Box::from_raw(source_ptr) }
    }

    /// Access the wrapped struct
    pub fn wrapped(&self) -> &G {
        // Safety: this is always a Some() until Drop is called
        unsafe { self.derived.as_ref().unwrap_unchecked() }
    }

    /// Modify the wrapped struct.
    /// # Safety
    /// It's possible to swap the derived objects of two Bound objects, which is UB.
    /// Mutation must not extend to the reference G holds to L.
    pub unsafe fn wrapped_mut(&mut self) -> &mut G {
        // Safety: this is always a Some() until Drop is called
        unsafe { self.derived.as_mut().unwrap_unchecked() }
    }
}

impl<G, L: 'static> Bound<G, L> {
    
    // region: constructors
    
    /// Bind data to a referrent
    pub fn new<F: FnOnce(&'static mut L) -> G>(source: L, func: F) -> Self {
        // We need to create the box to avoid strange allocators
        let source_ref = Box::leak(Box::new(source));
        let source_ptr = source_ref as *mut L;
        let derived = func(source_ref);
        Self { derived: Some(derived), source_ptr }
    }

    /// Bind data to a referrent or a referrent error
    pub fn try_new<E, F>(source: L, func: F) -> Result<Self, Bound<E, L>>
    where F: FnOnce(&'static mut L) -> Result<G, E>{
        let source_ref = Box::leak(Box::new(source));
        let source_ptr = source_ref as *mut L;
        func(source_ref)
            .map(|res| Bound{ derived: Some(res), source_ptr })
            .map_err(|e| Bound{ derived: Some(e), source_ptr })
    }

    /// Bind data to an asynchronously obtained referrent
    pub fn async_new<Fut: Send, F>(source: L, func: F)
    -> impl Future<Output = Self> + Send
    where F: FnOnce(&'static mut L) -> Fut, Fut: IntoFuture<Output = G>,
    <Fut as IntoFuture>::IntoFuture: Send, G: Send, L: Send {
        let source_ref = Box::leak(Box::new(source));
        let source_ptr = source_ref as *mut L;
        let mut instance = Self { source_ptr, derived: None };
        let fut = func(source_ref);
        async move {
            let derived = fut.await;
            instance.derived = Some(derived);
            instance
        }
    }

    /// Bind data to an asynchronously obtained referrent or referrent error
    pub fn async_try_new<E: Send, Fut: Send, F: Send>(source: L, func: F)
    -> impl Future<Output = Result<Self, Bound<E, L>>> + Send
    where F: FnOnce(&'static mut L) -> Fut, Fut: IntoFuture<Output = Result<G, E>>,
    <Fut as IntoFuture>::IntoFuture: Send, G: Send, L: Send {
        let source_ref = Box::leak(Box::new(source));
        let source_ptr = source_ref as *mut L;
        let fut = func(source_ref);
        let sendable_source_ptr = unsafe { source_ptr.as_mut().unwrap() };
        async move {
            let res = fut.await;
            let source_ptr = sendable_source_ptr as *mut L;
            res.map(|derived| Bound{ derived: Some(derived), source_ptr })
                .map_err(|e| Bound{ derived: Some(e), source_ptr })
        }
    }
    
    // endregion: constructors

    // region: map

    /// Replace the referrent
    pub fn map<G2, F>(self, func: F) -> Bound<G2, L> where F: FnOnce(G) -> G2 {
        let (derived, source_ptr) = self.decompose();
        let new = func(derived);
        Bound { derived: Some(new), source_ptr }
    }

    /// Replace the referrent or produce a referrent error
    pub fn try_map<G2, E, F>(self, func: F) -> Result<Bound<G2, L>, Bound<E, L>>
    where F: FnOnce(G) -> Result<G2, E> {
        let (derived, source_ptr) = self.decompose();
        func(derived)
            .map(|new| Bound { derived: Some(new), source_ptr })
            .map_err(|e| Bound { derived: Some(e), source_ptr })
    }

    /// Asynchronously replace the referrent
    pub fn async_map<G2: Send, Fut: Send, F>(self, func: F)
    -> impl Future<Output = Bound<G2, L>> + Send
    where F: FnOnce(G) -> Fut, Fut: IntoFuture<Output = G2>,
    <Fut as IntoFuture>::IntoFuture: Send, L: Send {
        let (derived, source_ptr) = self.decompose();
        let fut = func(derived);
        let mut next_instance = Bound { derived: None, source_ptr };
        async move {
            let new = fut.await;
            next_instance.derived = Some(new);
            next_instance
        }
    }

    /// Asynchronously replace the referrent or produce a referrent error
    pub fn async_try_map<G2: Send, E: Send, Fut: Send, F>(self, func: F)
    -> impl Future<Output = Result<Bound<G2, L>, Bound<E, L>>> + Send
    where F: FnOnce(G) -> Fut, Fut: IntoFuture<Output = Result<G2, E>>,
    <Fut as IntoFuture>::IntoFuture: Send, L: Send {
        let (derived, source_ptr) = self.decompose();
        let fut = func(derived);
        let sendable_source_ptr = unsafe { source_ptr.as_mut().unwrap() };
        async move {
            let res = fut.await;
            let source_ptr = sendable_source_ptr as *mut L;
            res.map(|new| Bound { derived: Some(new), source_ptr })
                .map_err(|e| Bound { derived: Some(e), source_ptr })
        }
    }

    // endregion: map
}

// This object only exposes G, so it's safe wherever holding a G is safe.
unsafe impl<G, L> Send for Bound<G, L> where G: Send {}
unsafe impl<G, L> Sync for Bound<G, L> where G: Sync {}

/// Ensure that
/// - `derived` gets dropped before `source_ptr`
/// - `source_ptr` gets dropped
impl<G, L> Drop for Bound<G, L> {
    fn drop(&mut self) {
        mem::drop(self.derived.take());
        // Bound::decompose might take the U out of the struct and set the pointer to null
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
        // Safety: it is assumed that this reference is somehow held by L and not
        // the other way around.
        unsafe { self.wrapped_mut().deref_mut() }
    }
}

impl<T: ?Sized, G, L> AsRef<T> for Bound<G, L> where G: AsRef<T> {
    fn as_ref(&self) -> &T { self.wrapped().as_ref() }
}

impl<T: ?Sized, G, L> AsMut<T> for Bound<G, L> where G: DerefMut + AsMut<T> {
    fn as_mut(&mut self) -> &mut T {
        // Safety: it is assumed that this reference is somehow held by L and not
        // the other way around.
        unsafe { self.wrapped_mut().as_mut() }
    }
}

#[cfg(test)]
mod tests {
    use std::ops::Deref;
    use std::sync::{Arc, RwLock, RwLockWriteGuard};

    use async_lock::RwLock as ARwLock;

    use super::*;

    struct MockStore(&'static str);
    impl Drop for MockStore {
        fn drop(&mut self) {
            println!("Store {} dropped", self.0);
            self.0 = "FAIL"; // Contaminate string so later assertions will fail
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

    // /// This is a compile-time error (but we can't test that)
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

    /// Alternative lock for mapping test
    struct MockLock2<'a>(&'a MockStore);
    impl Drop for MockLock2<'_> {
        fn drop(&mut self) { println!("Lock2 for {} dropped", self.0.0) }
    }

    #[test]
    fn test_map() {
        println!("-- testing mapping one type of lock into another --");
        let first = mk_lock();
        let second = first.map(|MockLock(store)| MockLock2(store));
        println!("The first lock should be gcd now");
        assert_eq!(second.wrapped().0.0, "flag");
    }

    /// This is a compilation test to make sure the future is always send
    #[allow(unused)]
    fn works_with_async_lock() -> impl Send {
        let lock = Arc::new(ARwLock::new(1));
        Bound::async_new(lock.clone(), |l| l.read())
    }

    #[allow(unused)]
    fn works_with_async_lock_of_async_lock() -> impl Send {
        let lock = Arc::new(ARwLock::new(Arc::new(ARwLock::new(1))));
        async move {
            let inner = Bound::async_new(lock.clone(), |l| l.read()).await;
            Bound::async_new(inner, |g| g.read())
        }
    }

    // /// The object returned by this function is corrupted, therefore wrapped_mut must not be public
    // #[allow(unused)]
    // fn swapping_must_not_compile() -> impl Send {
    //     let mut first = mk_lock();
    //     let mut second = mk_lock();
    //     mem::swap(first.wrapped_mut(), second.wrapped_mut());
    //     first
    // }
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