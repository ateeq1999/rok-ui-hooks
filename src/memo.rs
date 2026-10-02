//! Memos — lazily cached derived values: `use_memo`, `use_memo_eq`,
//! `create_memo`.

use std::cell::RefCell;
use std::rc::Rc;

use crate::deps::{Deps, DepsBox};
use crate::runtime::{Node, update_if_necessary};

/// A memo's cached value.
///
/// The slot is empty only between construction and the mount compute, which
/// happens inside [`use_memo_eq`] before the handle escapes.
pub(crate) struct MemoCell<T> {
    value: RefCell<Option<T>>,
}

impl<T> MemoCell<T> {
    fn new() -> Self {
        MemoCell {
            value: RefCell::new(None),
        }
    }

    fn borrow_value(&self) -> std::cell::Ref<'_, T> {
        std::cell::Ref::map(self.value.borrow(), |slot| {
            slot.as_ref().expect("memo read before its first compute")
        })
    }
}

/// A cached derived value.
///
/// A memo is a graph node, not a running effect: nothing recomputes until
/// somebody reads it, and readers only wake up when the value really changed.
pub struct Memo<T> {
    node: Rc<Node>,
    cell: Rc<MemoCell<T>>,
}

impl<T> Clone for Memo<T> {
    fn clone(&self) -> Self {
        Self {
            node: self.node.clone(),
            cell: self.cell.clone(),
        }
    }
}

impl<T: std::fmt::Debug> std::fmt::Debug for Memo<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Memo")
            .field("id", &self.node.id)
            .field("value", &*self.cell.borrow_value())
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
    let cell = Rc::new(MemoCell::new());
    let node = Node::create(false);
    node.set_deps(Rc::new(DepsBox(deps)));

    let target = cell.clone();
    let eq = Rc::new(eq);
    node.set_compute(Box::new(move || {
        let next = f();
        let mut current = target.value.borrow_mut();
        let changed = match current.as_ref() {
            Some(current) => !eq(&next, current),
            None => true,
        };
        if changed {
            *current = Some(next);
        }
        changed
    }));

    // Mount computes the value once, records the sources and the deps gate, so
    // the handle is usable the moment this returns.
    Node::mount(&node);

    Memo { node, cell }
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
        Node::track(&self.node);
        update_if_necessary(&self.node);
    }

    /// Tracked read by reference (no clone needed).
    pub fn with<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        self.prepare();
        f(&self.cell.borrow_value())
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
        T::clone(&self.cell.borrow_value())
    }

    /// Read the cached value by reference, without recomputing.
    pub fn with_untracked<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        f(&self.cell.borrow_value())
    }

    /// The cached value, without recomputing or subscribing.
    pub fn get_untracked(&self) -> T
    where
        T: Clone,
    {
        self.with_untracked(T::clone)
    }

    /// Force the next read to recompute, even if no dependency changed.
    ///
    /// Useful when the value came from outside the graph.
    pub fn invalidate(&self) {
        self.node.invalidate();
    }

    /// Stop this memo: reads keep returning the last cached value, but the memo
    /// no longer tracks anything. Dropping the last handle does the same.
    pub fn dispose(&self) {
        crate::runtime::dispose_node(self.node.id);
    }
}
