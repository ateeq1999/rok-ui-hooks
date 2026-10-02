//! A Zustand-style store: state outside any component tree, subscribed to by
//! hooks.
//!
//! ```
//! use signals::*;
//!
//! #[derive(Clone, PartialEq)]
//! struct App { count: i32 }
//!
//! let store = create_store(App { count: 0 });
//!
//! // like `useStore(store, s => s.count)` — a tracked selector read
//! let _render = use_effect(
//!     {
//!         let store = store.clone();
//!         move || println!("count = {}", use_store(&store, |s| s.count))
//!     },
//!     (),
//! );
//!
//! // like `store.subscribe((state, prev) => …)` — a plain listener
//! let sub = store.subscribe(|next: &App, prev: &App| println!("{} -> {}", prev.count, next.count));
//!
//! store.update(|s| s.count += 1);
//! drop(sub);
//! ```

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::Rc;

use crate::memo::{Memo, use_memo, use_memo_eq};
use crate::runtime::notify;
use crate::signal::{ReadSignal, WriteSignal, create_signal};

type Listener<T> = Box<dyn FnMut(&T, &T)>;

/// A reactive state container: readable and writable like a signal, observable by
/// plain listeners, and selectable with [`Store::select`].
pub struct Store<T> {
    read: ReadSignal<T>,
    write: WriteSignal<T>,
    listeners: Rc<RefCell<BTreeMap<u64, Listener<T>>>>,
    next_listener_id: Rc<Cell<u64>>,
}

impl<T> Clone for Store<T> {
    fn clone(&self) -> Self {
        Self {
            read: self.read.clone(),
            write: self.write.clone(),
            listeners: self.listeners.clone(),
            next_listener_id: self.next_listener_id.clone(),
        }
    }
}

impl<T: std::fmt::Debug + 'static> std::fmt::Debug for Store<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let value = self.read.with_untracked(|value| format!("{value:?}"));
        f.debug_struct("Store").field("value", &value).finish()
    }
}

/// Handle returned by [`Store::subscribe`]; dropping it unsubscribes.
pub struct Subscription<T> {
    listeners: Rc<RefCell<BTreeMap<u64, Listener<T>>>>,
    id: u64,
}

impl<T> std::fmt::Debug for Subscription<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Subscription")
            .field("id", &self.id)
            .finish()
    }
}

impl<T> Drop for Subscription<T> {
    fn drop(&mut self) {
        self.listeners.borrow_mut().remove(&self.id);
    }
}

/// `create(fn)` → a store holding the initial state.
pub fn create_store<T: 'static>(initial: T) -> Store<T> {
    let (read, write) = create_signal(initial);
    Store {
        read,
        write,
        listeners: Rc::new(RefCell::new(BTreeMap::new())),
        next_listener_id: Rc::new(Cell::new(0)),
    }
}

impl<T: 'static> Store<T> {
    /// `getState()` — tracked read, so it can be used inside an effect body.
    pub fn get(&self) -> T
    where
        T: Clone,
    {
        self.read.get()
    }

    /// Tracked read by reference.
    pub fn with<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        self.read.with(f)
    }

    /// Read without subscribing the current observer.
    pub fn with_untracked<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        self.read.with_untracked(f)
    }

    /// An untracked read, cloned.
    pub fn peek(&self) -> T
    where
        T: Clone,
    {
        self.read.get_untracked()
    }

    /// `setState(value)` — replace, fire the listeners, then notify reactions.
    pub fn set(&self, value: T) {
        let prev = self.write.replace(value);
        self.fire(&prev);
        notify(self.write.node());
    }

    /// `setState(prev => next)` — mutate in place. Requires `T: Clone` so the
    /// previous state can be handed to the listeners.
    pub fn update(&self, f: impl FnOnce(&mut T))
    where
        T: Clone,
    {
        let prev = self.read.get_untracked();
        self.write.update(f);
        self.fire(&prev);
    }

    /// `store.subscribe(listener)` — a plain callback on every change,
    /// independent of any effect. Returns a handle that unsubscribes on drop.
    ///
    /// The listener receives `(new, old)`.
    ///
    /// Listeners run while the new value is borrowed: do not call `set`/`update`
    /// on the same store from inside a listener.
    pub fn subscribe(&self, listener: impl FnMut(&T, &T) + 'static) -> Subscription<T> {
        let id = self.next_listener_id.get();
        self.next_listener_id.set(id + 1);
        self.listeners.borrow_mut().insert(id, Box::new(listener));
        Subscription {
            listeners: self.listeners.clone(),
            id,
        }
    }

    /// `createSelector(store, selector)` — a memo over a slice of the store.
    ///
    /// The selector runs at most once per change, and the memo only notifies when
    /// the slice it selected actually differs.
    ///
    /// ```
    /// use signals::*;
    ///
    /// #[derive(Clone, PartialEq)]
    /// struct App { count: i32, name: String }
    ///
    /// let store = create_store(App { count: 0, name: "Ada".into() });
    /// let name = store.select(|s: &App| s.name.clone());
    ///
    /// store.update(|s| s.count += 1); // the selector re-runs, the value does not change
    /// assert_eq!(name.get(), "Ada");
    /// ```
    pub fn select<U: PartialEq + 'static>(&self, selector: impl Fn(&T) -> U + 'static) -> Memo<U> {
        let store = self.clone();
        use_memo(move || store.with(|s| selector(s)), ())
    }

    /// [`Store::select`] with a custom equality test — Zustand's `useShallow`,
    /// and the way to bail out of a selector whose `PartialEq` is too strict.
    pub fn select_with_eq<U: 'static>(
        &self,
        selector: impl Fn(&T) -> U + 'static,
        eq: impl Fn(&U, &U) -> bool + 'static,
    ) -> Memo<U> {
        let store = self.clone();
        use_memo_eq(move || store.with(|s| selector(s)), (), eq)
    }

    fn fire(&self, prev: &T) {
        if self.listeners.borrow().is_empty() {
            return;
        }
        self.read.with_untracked(|next| {
            for listener in self.listeners.borrow_mut().values_mut() {
                listener(next, prev);
            }
        });
    }
}

/// `useStore(store, selector)` — a tracked selector read. Use it inside an effect
/// (or any observer) and the observer re-runs whenever the store changes.
pub fn use_store<T: 'static, U>(store: &Store<T>, selector: impl FnOnce(&T) -> U) -> U {
    store.with(selector)
}
