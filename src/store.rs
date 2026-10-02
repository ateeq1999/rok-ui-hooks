//! A Zustand-style store: state outside the component tree, subscribed to by
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

use crate::memo::{Memo, use_memo};
use crate::signal::{ReadSignal, WriteSignal, create_signal};

type Listener<T> = Box<dyn FnMut(&T, &T)>;

/// A reactive state container: readable/writable like a signal, observable by
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

/// Handle returned by [`Store::subscribe`]; dropping it unsubscribes.
pub struct Subscription<T> {
    listeners: Rc<RefCell<BTreeMap<u64, Listener<T>>>>,
    id: u64,
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

    pub fn with<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        self.read.with(f)
    }

    /// Read without subscribing the current observer.
    pub fn with_untracked<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        self.read.with_untracked(f)
    }

    /// `setState(value)` — replace, notify listeners, then notify reactions.
    pub fn set(&self, value: T) {
        let prev = self.write.inner.write(value);
        self.fire(&prev);
        self.write.inner.notify();
    }

    /// `setState(prev => next)` — mutate in place. Requires `T: Clone` so the
    /// previous state can be handed to listeners.
    pub fn update(&self, f: impl FnOnce(&mut T))
    where
        T: Clone,
    {
        let prev = self.read.get_untracked();
        self.write.update(|value| f(value));
        self.fire(&prev);
    }

    /// `store.subscribe(listener)` — a plain callback on every change,
    /// independent of any effect. Returns a handle that unsubscribes on drop.
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
    /// Put the returned handle in a deps tuple to get Zustand's bail-out: the
    /// reader only re-runs when this slice actually changes.
    pub fn select<U: PartialEq + 'static>(&self, selector: impl Fn(&T) -> U + 'static) -> Memo<U>
    where
        T: Clone,
    {
        let store = self.clone();
        use_memo(move || store.with(|s| selector(s)), ())
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

/// `useStore(store, selector)` — a tracked selector read. Use it inside an
/// effect (or any observer) and the observer re-runs whenever the store changes.
pub fn use_store<T: 'static, U>(store: &Store<T>, selector: impl FnOnce(&T) -> U) -> U {
    store.with(selector)
}
