//! Signals — the state primitive, exposed as `use_state`.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::{Rc, Weak};

use crate::runtime::{Computation, ComputationId, Source, batch, current_observer};

pub(crate) struct SignalInner<T> {
    pub(crate) value: RefCell<T>,
    /// Ordered by id so effects re-run in creation order (deterministic).
    subscribers: RefCell<BTreeMap<ComputationId, Weak<Computation>>>,
}

impl<T> Source for SignalInner<T> {
    fn unsubscribe(&self, id: ComputationId) {
        self.subscribers.borrow_mut().remove(&id);
    }
}

impl<T: 'static> SignalInner<T> {
    /// Link the current observer to this signal — both ways.
    fn track(self: &Rc<Self>) {
        if let Some(running) = current_observer() {
            let newly_added = self
                .subscribers
                .borrow_mut()
                .insert(running.id, Rc::downgrade(&running))
                .is_none();
            if newly_added {
                running
                    .sources
                    .borrow_mut()
                    .push(self.clone() as Rc<dyn Source>);
            }
        }
    }

    /// Notify subscribers inside a transaction. Snapshot first: running a
    /// subscriber mutates the subscriber list.
    pub(crate) fn notify(&self) {
        let subs: Vec<Rc<Computation>> = {
            let mut map = self.subscribers.borrow_mut();
            map.retain(|_, w| w.strong_count() > 0);
            map.values().filter_map(Weak::upgrade).collect()
        };
        batch(|| {
            for sub in subs {
                sub.schedule();
            }
        });
    }
}

/// The read half of a signal — you cannot write through it.
pub struct ReadSignal<T> {
    pub(crate) inner: Rc<SignalInner<T>>,
}

/// The write half of a signal — you cannot read a tracked value through it.
pub struct WriteSignal<T> {
    pub(crate) inner: Rc<SignalInner<T>>,
}

impl<T> Clone for ReadSignal<T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}
impl<T> Clone for WriteSignal<T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

pub(crate) fn create_signal<T: 'static>(value: T) -> (ReadSignal<T>, WriteSignal<T>) {
    let inner = Rc::new(SignalInner {
        value: RefCell::new(value),
        subscribers: RefCell::new(BTreeMap::new()),
    });
    (
        ReadSignal {
            inner: inner.clone(),
        },
        WriteSignal { inner },
    )
}

/// `useState(initial)` → `(state, set_state)`.
pub fn use_state<T: 'static>(initial: T) -> (ReadSignal<T>, WriteSignal<T>) {
    create_signal(initial)
}

impl<T: 'static> ReadSignal<T> {
    /// Tracked read by reference (no clone needed).
    pub fn with<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        self.inner.track();
        f(&self.inner.value.borrow())
    }

    /// Tracked read that clones the value.
    pub fn get(&self) -> T
    where
        T: Clone,
    {
        self.with(T::clone)
    }

    /// Read without subscribing the current observer.
    pub fn with_untracked<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        f(&self.inner.value.borrow())
    }

    pub fn get_untracked(&self) -> T
    where
        T: Clone,
    {
        self.with_untracked(T::clone)
    }
}

impl<T: 'static> WriteSignal<T> {
    /// Replace the value and notify. The value borrow is released before
    /// notifying, so effects can read the new value.
    pub fn set(&self, value: T) {
        self.inner.write(value);
        self.inner.notify();
    }

    /// Mutate in place (great for `Vec` / `HashMap` signals).
    pub fn update(&self, f: impl FnOnce(&mut T)) {
        f(&mut self.inner.value.borrow_mut());
        self.inner.notify();
    }
}

impl<T: 'static> SignalInner<T> {
    /// Assign without notifying — used by the store, which fires its own
    /// subscribers (plain listeners) first.
    pub(crate) fn write(&self, value: T) -> T {
        std::mem::replace(&mut *self.value.borrow_mut(), value)
    }
}
