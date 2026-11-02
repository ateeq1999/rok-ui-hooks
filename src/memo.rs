//! Memos — lazily cached derived values: `use_memo`, `use_memo_eq`,
//! `create_memo`.

use std::rc::Rc;

use crate::deps::{Deps, DepsBox};
use crate::runtime::{Node, update_if_necessary};
use crate::shared::{Handle, Slot, downgrade};

/// A memo's cached value.
///
/// The slot is empty only between construction and the mount compute, which
/// happens inside [`use_memo_eq`] before the handle escapes.
pub(crate) struct MemoCell<T> {
    value: Slot<Option<T>>,
}

impl<T> MemoCell<T> {
    fn new() -> Self {
        MemoCell {
            value: Slot::new(None),
        }
    }

    /// Read the cached value. Panics if the memo is read before its first
    /// compute, which cannot happen through the public API.
    fn with_value<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        self.value
            .with(|slot| f(slot.as_ref().expect("memo read before its first compute")))
    }
}

/// A cached derived value.
///
/// A memo is a graph node, not a running effect: nothing recomputes until
/// somebody reads it, and readers only wake up when the value really changed.
///
/// With the `thread-safe` feature a `Memo<T>` is `Send + Sync` whenever `T` is,
/// and a read on a thread that does not own the graph returns the last value
/// computed there — a memo cannot recompute away from its graph, so think of the
/// handle as a snapshot rather than a live derived value on another thread.
pub struct Memo<T> {
    /// Strong by default: a tracked read needs the node, and a refcount bump is
    /// cheaper than a registry lookup. Under `thread-safe` the handle must not
    /// hold an `Rc`, so it addresses the node by id.
    #[cfg(not(feature = "thread-safe"))]
    node: Rc<Node>,
    #[cfg(feature = "thread-safe")]
    id: u64,
    cell: Handle<MemoCell<T>>,
}

impl<T> Clone for Memo<T> {
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

/// The node behind a memo handle, or `None` on a thread that does not own it.
impl<T> Memo<T> {
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

impl<T: std::fmt::Debug> std::fmt::Debug for Memo<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        #[cfg(not(feature = "thread-safe"))]
        let id = self.node.id;
        #[cfg(feature = "thread-safe")]
        let id = self.id;
        f.debug_struct("Memo")
            .field("id", &id)
            .field(
                "value",
                &self.cell.with_value(|value| format!("{value:?}")),
            )
            .finish()
    }
}

/// `useMemo(fn, deps)` — recompute `f` when a dependency changes, and notify
/// readers only when the produced value actually differs.
///
/// Pass `()` for automatic fine-grained tracking, or a tuple of handles for
/// React-style declared dependencies — see [`Deps`].
///
/// The body runs exactly once at creation, so the value is available
/// immediately; after that it runs only when a dependency moved *and* the new
/// value differs.
///
/// ```
/// use rok_ui_hooks::*;
///
/// let (first, set_first) = use_state("John".to_string());
/// let (last, _) = use_state("Smith".to_string());
/// let full = use_memo(
///     {
///         let (first, last) = (first.clone(), last.clone());
///         move || format!("{} {}", first.get(), last.get())
///     },
///     (),
/// );
///
/// assert_eq!(full.get(), "John Smith");
/// set_first.set("Jacob".into());
/// assert_eq!(full.get(), "Jacob Smith");
/// ```
pub fn use_memo<T: PartialEq + 'static>(
    f: impl FnMut() -> T + 'static,
    deps: impl Deps + 'static,
) -> Memo<T> {
    use_memo_eq(f, deps, |next, current| next == current)
}

/// `useMemo(fn, deps)` with your own equality test.
///
/// The default memo bails out with [`PartialEq`]. For values whose `PartialEq`
/// is too strict or too lax (a `Vec` rebuilt on every store write, say) pass a
/// comparator — see [`shallow_vec_eq`].
///
/// ```
/// use rok_ui_hooks::*;
///
/// let (n, set_n) = use_state(2);
/// let doubled = use_memo_eq(
///     { let n = n.clone(); move || n.get() * 2 },
///     (),
///     |next: &i32, current: &i32| next.abs() == current.abs(),
/// );
///
/// set_n.set(-2);
/// assert_eq!(doubled.get(), 4); // -4 equals 4 by the custom test: nobody is notified
/// set_n.set(3);
/// assert_eq!(doubled.get(), 6);
/// ```
pub fn use_memo_eq<T: 'static>(
    mut f: impl FnMut() -> T + 'static,
    deps: impl Deps + 'static,
    eq: impl Fn(&T, &T) -> bool + 'static,
) -> Memo<T> {
    let cell: Handle<MemoCell<T>> = Handle::new(MemoCell::new());
    let node = Node::create(false);
    node.set_deps(Rc::new(DepsBox(deps)));

    // The node reaches its own value through a weak pointer: the handle decides
    // when the value dies, and a graph node must not extend it. Under
    // `thread-safe` this also keeps `Arc::strong_count` a truthful answer to
    // "is this the last handle?", which is how the node is freed again.
    let target = downgrade(&cell);
    let eq = Rc::new(eq);
    node.set_compute(Box::new(move || {
        let next = f();
        let Some(target) = target.upgrade() else {
            return false; // the value is gone; so is every reader of it
        };
        target.value.with_mut(|current| {
            let changed = match current.as_ref() {
                Some(current) => !eq(&next, current),
                None => true,
            };
            if changed {
                *current = Some(next);
            }
            changed
        })
    }));

    // Mount computes the value once, records the sources and the deps gate, so
    // the handle is usable the moment this returns.
    Node::mount(&node);

    let id = node.id;
    #[cfg(feature = "thread-safe")]
    crate::runtime::keep_alive(id, &node);
    #[cfg(not(feature = "thread-safe"))]
    let memo = Memo { node, cell };
    #[cfg(feature = "thread-safe")]
    let memo = Memo { id, cell };
    memo
}

/// The last handle to a memo gives its node back.
///
/// Without this the node would outlive the value it computes, which under
/// `thread-safe` means an allocation nothing can ever read again. `is_last` is
/// checked against the *value* cell rather than a node refcount because the
/// handle is the only owner of both.
#[cfg(feature = "thread-safe")]
impl<T> Drop for Memo<T> {
    fn drop(&mut self) {
        if crate::shared::is_last(&self.cell) {
            crate::runtime::release_keep_alive(self.id);
        }
    }
}

/// `createMemo(fn, deps)` — the Solid-flavoured name of [`use_memo`].
///
/// In Rust there is no component to run "during", so `create_*` and `use_*` are
/// the same function; the pair exists to keep code portable.
pub fn create_memo<T: PartialEq + 'static>(
    f: impl FnMut() -> T + 'static,
    deps: impl Deps + 'static,
) -> Memo<T> {
    use_memo(f, deps)
}

/// Element-wise equality for `Vec`s, whatever their capacity and pointer.
pub fn shallow_vec_eq<T: PartialEq>(next: &Vec<T>, current: &Vec<T>) -> bool {
    next == current
}

/// Element-wise equality for fixed-size arrays.
pub fn shallow_array_eq<T: PartialEq, const N: usize>(next: &[T; N], current: &[T; N]) -> bool {
    next == current
}

impl<T: 'static> Memo<T> {
    /// Subscribe the current observer and make sure the value is fresh.
    fn prepare(&self) {
        if let Some(node) = self.node() {
            Node::track(&node);
            update_if_necessary(&node);
        }
    }

    /// Tracked read by reference (no clone needed).
    pub fn with<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        self.prepare();
        self.cell.with_value(f)
    }

    /// Tracked read that clones the value.
    pub fn get(&self) -> T
    where
        T: Clone,
    {
        self.with(T::clone)
    }

    /// Read the cached value without recomputing or subscribing.
    pub fn peek(&self) -> T
    where
        T: Clone,
    {
        self.cell.with_value(T::clone)
    }

    /// Read the cached value by reference, without recomputing.
    pub fn with_untracked<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        self.cell.with_value(f)
    }

    /// The cached value, without recomputing or subscribing.
    pub fn get_untracked(&self) -> T
    where
        T: Clone,
    {
        self.with_untracked(T::clone)
    }

    /// The id of the node behind this handle — the same value
    /// [`ReadSignal::id`](crate::ReadSignal::id) reports for a signal.
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

    /// Force the next read to recompute, even if no dependency changed.
    ///
    /// Useful when the value came from outside the graph.
    pub fn invalidate(&self) {
        if let Some(node) = self.node() {
            node.invalidate();
        }
    }

    /// Stop this memo: reads keep returning the last cached value, but the memo
    /// no longer tracks anything. Dropping the last handle does the same.
    pub fn dispose(&self) {
        crate::runtime::dispose_node(self.id());
    }
}
