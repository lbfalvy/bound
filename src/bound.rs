use std::fmt::Display;
use std::future::IntoFuture;
use std::ops::Deref;
use std::ops::DerefMut;
use std::mem;
use std::ptr;
use std::fmt::Debug;
use std::hash::Hash;


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
pub struct Bound<T, U> {
    // Needed so that Drop can consume the T.
    // Safety: this will always be Some() until Drop is called
    derived: Option<T>,
    source_ptr: *mut U,
}

impl<'a, T, U: 'a> Bound<T, U> {
    /// Bind data to a referrent
    pub fn new<F: FnOnce(&'a mut U) -> T>(source: U, func: F) -> Self {
        // We need to create the box to avoid strange allocators
        let source_ref = Box::leak(Box::new(source));
        let source_ptr = source_ref as *mut U;
        let derived = func(source_ref);
        Self {
            derived: Some(derived),
            source_ptr
        }
    }

    /// Bind data to a referrent or a referrent error
    pub fn try_new<E, F>(source: U, func: F) -> Result<Self, Bound<E, U>>
    where F: FnOnce(&'a mut U) -> Result<T, E>{
        let source_ref = Box::leak(Box::new(source));
        let source_ptr = source_ref as *mut U;
        func(source_ref)
            .map(|res| Bound{ derived: Some(res), source_ptr })
            .map_err(|e| Bound{ derived: Some(e), source_ptr })
    }

    /// Bind data to an asynchronously obtained referrent
    pub async fn async_new<Fut, F>(source: U, func: F) -> Self
    where Fut: IntoFuture<Output = T>, F: FnOnce(&'a mut U) -> Fut {
        let source_ref = Box::leak(Box::new(source));
        let source_ptr = source_ref as *mut U;
        let derived = func(source_ref).await;
        Self {
            derived: Some(derived),
            source_ptr
        }
    }

    /// Bind data to an asynchronously obtained referrent or referrent error
    pub async fn async_try_new<E, Fut, F>(source: U, func: F) -> Result<Self, Bound<E, U>>
    where Fut: IntoFuture<Output = Result<T, E>>, F: FnOnce(&'a mut U) -> Fut {
        let source_ref = Box::leak(Box::new(source));
        let source_ptr = source_ref as *mut U;
        func(source_ref).await
            .map(|res| Bound{ derived: Some(res), source_ptr })
            .map_err(|e| Bound{ derived: Some(e), source_ptr })
    }

    /// Drop T and get back U
    pub fn unbind(mut self) -> Box<U> {
        mem::drop(self.derived.take());
        // Safety: a mutable reference to source was held by derived which was dropped just above
        // in addition, this function consumes self so it will only run once, and this is the
        // only place where source_ptr is set to null, so checking for null here is not necessary.
        let source = unsafe { Box::from_raw(self.source_ptr) };
        self.source_ptr = ptr::null_mut(); // Prevent Drop from deallocating source
        source
    }
}

/// Ensure that
/// - `derived` gets dropped before `source_ptr`
/// - `source_ptr` gets dropped
impl<T, U> Drop for Bound<T, U> {
    fn drop(&mut self) {
        mem::drop(self.derived.take());
        if !self.source_ptr.is_null() {
            // Safety: A mutable reference to source was held by derived, but derived was dropped
            // just above therefore the Box may safely be dropped.
            let source = unsafe { Box::from_raw(self.source_ptr) };
            mem::drop(source)
        };
    }
}

impl<T, U> Deref for Bound<T, U> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        unsafe {
            self.derived.as_ref().unwrap_unchecked()
        }
    }
}

impl<T, U> DerefMut for Bound<T, U> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // Safety: this is always a Some() until Drop is called
        unsafe {
            self.derived.as_mut().unwrap_unchecked()
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, RwLock};

    use super::*;

    struct Mock<'a>(&'a str, Option<&'a Mock<'a>>);
    impl<'a> Drop for Mock<'a> {
        fn drop(&mut self) {
            let Mock(name, _) = self;
            println!("{name} dropped");
            self.0 = "FAIL";
        }
    }

    fn mk_source() -> Mock<'static> { Mock("source", None) }

    // /// This is a compile-time error (but we can't test that)
    // fn mk_derived_naiive() -> Mock { Mock("derived", Some(&mk_source())) }

    /// Basic usage of Bound as a smart pointer
    fn mk_derived() -> Bound<Mock<'static>, Mock<'static>> {
        Bound::new(mk_source(), |src| Mock("derived", Some(src)))
    }

    #[test]
    fn test_functionality() {
        let derived = mk_derived();
        assert_eq!(derived.1.unwrap().0, "source");
        assert_eq!(derived.0, "derived");
    }

    #[test]
    fn test_unbind() {
        let derived = mk_derived();
        let source = derived.unbind();
        println!("derived should be dropped but source is still alive");
        assert_eq!(source.0, "source");
    }

    #[test]
    fn test_rwlock() {
        let shared_data = Arc::new(RwLock::new(1));
        let mut writer = Bound::new(shared_data.clone(), |a| a.write().unwrap());
        **writer = 2;
    }
}

// Delegate std implementations to `self.deref()`

impl<T, U, V> AsRef<V> for Bound<T, U> where T: AsRef<V> {
    fn as_ref(&self) -> &V { self.deref().as_ref() }
}

impl<T, U, V> AsMut<V> for Bound<T, U> where T: AsMut<V> {
    fn as_mut(&mut self) -> &mut V { self.deref_mut().as_mut() }
}

impl<T, U> Debug for Bound<T, U> where T: Debug {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Bound")
            .field(self.deref())
            .finish()
    }
}

impl<T, U> Display for Bound<T, U> where T: Display {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.deref().fmt(f)
    }
}

impl<T, U> Hash for Bound<T, U> where T: Hash {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) { self.deref().hash(state) }
}

impl<T, U> Eq for Bound<T, U> where T: Eq {}
impl<T, U, V, W> PartialEq<Bound<V, W>> for Bound<T, U> where T: PartialEq<V> {
    fn eq(&self, other: &Bound<V, W>) -> bool { self.deref().eq(other.deref()) }
    fn ne(&self, other: &Bound<V, W>) -> bool { self.deref().ne(other.deref()) }
}

impl<T, U, V, W> PartialOrd<Bound<V, W>> for Bound<T, U> where T: PartialOrd<V> {
    fn partial_cmp(&self, other: &Bound<V, W>) -> Option<std::cmp::Ordering> {
        self.deref().partial_cmp(&other.deref())
    }
}

impl<T, U> Ord for Bound<T, U> where T: Ord {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering { self.deref().cmp(other.deref()) }
}