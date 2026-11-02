//! Signals — the state primitive, exposed as `use_state` / `create_signal`.

use std::rc::Rc;

use crate::runtime::{Id, Node, notify};
use crate::shared::{Handle, Slot};

/// The stored value. Kept apart from the [`Node`] so handles stay typed: the
/// graph is type-erased, the data is not.
pub(crate) type ValueCell<T> = Slot<T>;

/// The read half of a signal — you cannot write through it.
///
/// With the `thread-safe` feature this is the half that may cross a thread
/// boundary: `ReadSignal<T>` is `Send + Sync` whenever `T` is. A read on a thread
/// that does not own the graph is untracked, because there is no graph there to
/// subscribe to — the value it returns is still current, since every write goes
/// through the owning thread.
pub struct ReadSignal<T> {
    /// Kept as a strong reference by default: a tracked read needs the node, and
    /// a refcount bump is cheaper than a registry lookup. Under `thread-safe`
    /// the read half must not hold an `Rc`, so it addresses the node by id.
    #[cfg(not(feature = "thread-safe"))]
    node: Rc<Node>,
    #[cfg(feature = "thread-safe")]
    id: Id,
    cell: Handle<ValueCell<T>>,
}

/// The write half of a signal — you cannot read a tracked value through it.
///
/// Always `!Send`, with or without `thread-safe`: writes belong to the thread
/// that owns the graph.
pub struct WriteSignal<T> {
    node: Rc<Node>,
    cell: Handle<ValueCell<T>>,
}

impl<T> Clone for ReadSignal<T> {
    fn clone(&self) -> Self {
        #[cfg(not(feature = "thread-safe"))]
        {
            Self {
                node: self.node.clone(),
                cell: self.cell.clone(),
            }
        }
        #[cfg(feature = "thread-safe")]
        {
            Self {
                id: self.id,
                cell: self.cell.clone(),
            }
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

/// The node behind a read half, or `None` on a thread that does not own it.
impl<T> ReadSignal<T> {
    fn node(&self) -> Option<Rc<Node>> {
        #[cfg(not(feature = "thread-safe"))]
        {
            Some(self.node.clone())
        }
        #[cfg(feature = "thread-safe")]
        {
            crate::runtime::lookup(self.id)
        }
    }
}

impl<T: std::fmt::Debug> std::fmt::Debug for ReadSignal<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        #[cfg(not(feature = "thread-safe"))]
        let id = self.node.id;
        #[cfg(feature = "thread-safe")]
        let id = self.id;
        f.debug_struct("ReadSignal")
            .field("id", &id)
            .field("value", &self.cell.debug())
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
    let cell: Handle<ValueCell<T>> = Handle::new(Slot::new(initial));
    let read = {
        #[cfg(not(feature = "thread-safe"))]
        {
            ReadSignal {
                node: node.clone(),
                cell: cell.clone(),
            }
        }
        #[cfg(feature = "thread-safe")]
        {
            ReadSignal {
                id: node.id,
                cell: cell.clone(),
            }
        }
    };
    (read, WriteSignal { node, cell })
}

/// `useState(initial)` → `(state, set_state)`.
///
/// ```
/// use rok_ui_hooks::*;
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
        if let Some(node) = self.node() {
            Node::track(&node);
        }
        self.cell.with(f)
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
        self.cell.with(f)
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

    /// The id of the node behind this handle. Equal ids mean the same signal,
    /// which is what a debug view or a test needs.
    pub fn id(&self) -> u64 {
        #[cfg(not(feature = "thread-safe"))]
        {
            self.node.id
        }
        #[cfg(feature = "thread-safe")]
        {
            self.id
        }
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
        self.cell.with_mut(f);
        notify(&self.node);
    }

    /// Write the value without notifying, returning the previous one.
    pub(crate) fn replace(&self, value: T) -> T {
        self.cell.replace(value)
    }

    /// The node behind this handle, so a store can notify its subscribers.
    /// Only [`crate::Store`] does that, so it rides the `store` feature; a new
    /// caller must widen this `cfg`.
    #[cfg(feature = "store")]
    pub(crate) fn node(&self) -> &Rc<Node> {
        &self.node
    }
}