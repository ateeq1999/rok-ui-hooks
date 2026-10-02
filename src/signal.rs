//! Signals — the state primitive, exposed as `use_state` / `create_signal`.

use std::cell::RefCell;
use std::rc::Rc;

use crate::runtime::{Node, notify};

/// The stored value. Kept apart from the [`Node`] so handles stay typed: the
/// graph is type-erased, the data is not.
pub(crate) struct ValueCell<T> {
    pub(crate) value: RefCell<T>,
}

impl<T> ValueCell<T> {
    pub(crate) fn new(value: T) -> Self {
        ValueCell {
            value: RefCell::new(value),
        }
    }
}

/// The read half of a signal — you cannot write through it.
pub struct ReadSignal<T> {
    node: Rc<Node>,
    cell: Rc<ValueCell<T>>,
}

/// The write half of a signal — you cannot read a tracked value through it.
pub struct WriteSignal<T> {
    node: Rc<Node>,
    cell: Rc<ValueCell<T>>,
}

impl<T> Clone for ReadSignal<T> {
    fn clone(&self) -> Self {
        Self {
            node: self.node.clone(),
            cell: self.cell.clone(),
        }
    }
}

impl<T> Clone for WriteSignal<T> {
    fn clone(&self) -> Self {
        Self {
            node: self.node.clone(),
            cell: self.cell.clone(),
        }
    }
}

impl<T: std::fmt::Debug> std::fmt::Debug for ReadSignal<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReadSignal")
            .field("id", &self.node.id)
            .field("value", &*self.cell.value.borrow())
            .finish()
    }
}

impl<T> std::fmt::Debug for WriteSignal<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WriteSignal")
            .field("id", &self.node.id)
            .finish()
    }
}

/// `createSignal(v)` → `(read, write)`.
pub fn create_signal<T: 'static>(initial: T) -> (ReadSignal<T>, WriteSignal<T>) {
    let node = Node::create(false);
    let cell = Rc::new(ValueCell::new(initial));
    (
        ReadSignal {
            node: node.clone(),
            cell: cell.clone(),
        },
        WriteSignal { node, cell },
    )
}

/// `useState(initial)` → `(state, set_state)`.
///
/// ```
/// use signals::*;
///
/// let (count, set_count) = use_state(0);
/// let _e = use_effect(
///     { let count = count.clone(); move || println!("{}", count.get()) },
///     (),
/// );
/// set_count.set(1);
/// ```
pub fn use_state<T: 'static>(initial: T) -> (ReadSignal<T>, WriteSignal<T>) {
    create_signal(initial)
}

impl<T: 'static> ReadSignal<T> {
    /// Tracked read by reference (no clone needed).
    pub fn with<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        Node::track(&self.node);
        f(&self.cell.value.borrow())
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
        f(&self.cell.value.borrow())
    }

    /// Read without subscribing the current observer.
    pub fn get_untracked(&self) -> T
    where
        T: Clone,
    {
        self.with_untracked(T::clone)
    }

    /// Alias of [`ReadSignal::get_untracked`], to pair with [`Memo::peek`](crate::Memo::peek).
    pub fn peek(&self) -> T
    where
        T: Clone,
    {
        self.get_untracked()
    }
}

impl<T: 'static> WriteSignal<T> {
    /// Replace the value and notify. The borrow is released before notifying,
    /// so effects can read the new value.
    pub fn set(&self, value: T) {
        self.replace(value);
        notify(&self.node);
    }

    /// Mutate in place (handy for `Vec` / `HashMap` signals).
    pub fn update(&self, f: impl FnOnce(&mut T)) {
        f(&mut *self.cell.value.borrow_mut());
        notify(&self.node);
    }

    /// Write the value without notifying, returning the previous one.
    pub(crate) fn replace(&self, value: T) -> T {
        std::mem::replace(&mut *self.cell.value.borrow_mut(), value)
    }

    pub(crate) fn node(&self) -> &Rc<Node> {
        &self.node
    }
}
