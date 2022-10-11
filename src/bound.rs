use std::ops::Deref;
use std::ops::DerefMut;
use std::mem;
use std::ptr;

/// Derive a T from a U and use it safely. U is placed on the heap, then
/// a reference to it is used to construct T which is stored inline. This is not possible currently
/// within Rust's type system so we need to use unsafe code.
pub struct Bound<T, U> {
    // Needed so that Drop can consume the T.
    // Safety: this will always be Some() until Drop is called
    derived: Option<T>,
    source_ptr: *mut U,
}

impl<'a, T: 'a, U: 'a> Bound<T, U> {
    pub fn new<F: FnOnce(&'a mut U) -> T>(source: U, func: F) -> Self {
        // We need to create the box to avoid strange allocators
        let source_ref = Box::leak(Box::new(source));
        let source_ptr = source_ref as *mut U;
        let derived = func(source_ref);
        Self {
            derived: Some(derived),
            source_ptr: source_ptr
        }
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
impl<'a, T, U> Drop for Bound<T, U> {
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

/// Represent a Derived<'a, T...> as &'a T
impl<'a, T, U> Deref for Bound<T, U> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        unsafe {
            self.derived.as_ref().unwrap_unchecked()
        }
    }
}

/// Represent a Derived<'a, T...> as &'a mut T
impl<'a, T, U> DerefMut for Bound<T, U> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // Safety: this is always a Some() until Drop is called
        unsafe {
            self.derived.as_mut().unwrap_unchecked()
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    struct Mock(&'static str, Option<&'static Mock>);
    impl Drop for Mock {
        fn drop(&mut self) {
            let Mock(name, _) = self;
            println!("{name} dropped");
            self.0 = "FAIL";
        }
    }

    fn mk_source() -> Mock { Mock("source", None) }

    // /// This is a compile-time error (but we can't test that)
    // fn mk_derived_naiive() -> Mock { Mock("derived", Some(&mk_source())) }

    /// Basic usage of Bound as a smart pointer
    fn mk_derived() -> Bound<Mock, Mock> {
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
}
