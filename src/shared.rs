//! The one place where "where does a value live" is decided.
//!
//! A signal and a memo both need a value that several handles share. There are
//! two shapes, chosen by the `thread-safe` feature:
//!
//! | feature       | cell      | handle | `ReadSignal<T>` is         |
//! |---------------|-----------|--------|----------------------------|
//! | off (default) | `RefCell` | `Rc`   | `!Send`, `!Sync`           |
//! | on            | `RwLock`  | `Arc`  | `Send + Sync` if `T: Send + Sync` |
//!
//! The default is `Rc` + `RefCell` because that is what a single-threaded graph
//! wants: an unchecked borrow and a non-atomic refcount. `RwLock` + `Arc` would
//! make every read take a lock and every clone touch an atomic counter, which is
//! a real cost for the 99% case.
//!
//! Turning `thread-safe` on buys exactly one thing: the **read half** of a signal
//! or a memo can be moved to another thread and read there. That works because a
//! read half holds no node — only an id — so a thread that does not own the graph
//! finds nothing to subscribe to and simply reads the value. The value itself is
//! shared through the `Arc`, so it stays current. The write half keeps its `Rc`
//! and therefore stays `!Send`: there is exactly one way in.
//!
//! This is deliberately *not* the 0.4 engine. That one would let several threads
//! mutate one graph; this one lets another thread observe a value.

/// The cell behind a [`Slot`].
#[cfg(not(feature = "thread-safe"))]
type Raw<T> = std::cell::RefCell<T>;
#[cfg(feature = "thread-safe")]
type Raw<T> = std::sync::RwLock<T>;

/// A shared pointer to a value cell, whatever the feature is on.
#[cfg(not(feature = "thread-safe"))]
pub(crate) type Handle<T> = std::rc::Rc<T>;
#[cfg(feature = "thread-safe")]
pub(crate) type Handle<T> = std::sync::Arc<T>;

/// A non-owning pointer to a value cell. A graph node holds one of these for its
/// own value, so a node never keeps a value alive on its own: the caller's handle
/// decides when the value goes.
#[cfg(not(feature = "thread-safe"))]
pub(crate) type WeakHandle<T> = std::rc::Weak<T>;
#[cfg(feature = "thread-safe")]
pub(crate) type WeakHandle<T> = std::sync::Weak<T>;

/// A non-owning pointer to `handle`.
pub(crate) fn downgrade<T>(handle: &Handle<T>) -> WeakHandle<T> {
    #[cfg(not(feature = "thread-safe"))]
    {
        std::rc::Rc::downgrade(handle)
    }
    #[cfg(feature = "thread-safe")]
    {
        std::sync::Arc::downgrade(handle)
    }
}

/// `true` when `handle` is the last one pointing at its cell.
pub(crate) fn is_last<T>(handle: &Handle<T>) -> bool {
    #[cfg(not(feature = "thread-safe"))]
    {
        std::rc::Rc::strong_count(handle) == 1
    }
    #[cfg(feature = "thread-safe")]
    {
        std::sync::Arc::strong_count(handle) == 1
    }
}

/// A shared value cell with one access shape on both backends.
///
/// Both methods hand the closure a plain `&T` / `&mut T` and never let a guard
/// escape, so no caller has to care which cell it got.
pub(crate) struct Slot<T>(Raw<T>);

impl<T> Slot<T> {
    pub(crate) fn new(value: T) -> Self {
        #[cfg(not(feature = "thread-safe"))]
        {
            Slot(std::cell::RefCell::new(value))
        }
        #[cfg(feature = "thread-safe")]
        {
            Slot(std::sync::RwLock::new(value))
        }
    }

    pub(crate) fn with<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        #[cfg(not(feature = "thread-safe"))]
        {
            f(&self.0.borrow())
        }
        #[cfg(feature = "thread-safe")]
        {
            // A panic while the value was borrowed poisons the lock. The value is
            // still there and the graph is still consistent, so recovering beats
            // turning one bad effect into a dead runtime.
            let guard = self
                .0
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            f(&guard)
        }
    }

    pub(crate) fn with_mut<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
        #[cfg(not(feature = "thread-safe"))]
        {
            f(&mut self.0.borrow_mut())
        }
        #[cfg(feature = "thread-safe")]
        {
            let mut guard = self
                .0
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            f(&mut guard)
        }
    }

    /// Swap in `value`, handing back the one it replaced.
    pub(crate) fn replace(&self, value: T) -> T {
        self.with_mut(|current| std::mem::replace(current, value))
    }
}

impl<T: std::fmt::Debug> Slot<T> {
    pub(crate) fn debug(&self) -> String {
        self.with(|value| format!("{value:?}"))
    }
}