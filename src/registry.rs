//! Id → handle bookkeeping, in one place.
//!
//! Three parts of the graph need the same primitive: a map from a node id to the
//! handle that identifies it.
//!
//! * `Node::subs` — who reads this node.
//! * `REGISTRY` — how an id is turned back into a live node.
//! * `RUNS` — how many times one node has run inside a flush, to spot cycles.
//!
//! Keeping it here is not only about tidiness: it is the one piece of the graph
//! with a *concurrent* contract worth proving. `tests/loom_model.rs` compiles
//! this very file with `loom::sync` types and explores every interleaving of
//! concurrent insert / remove / snapshot, so the bookkeeping is checked rather
//! than assumed. Build it with `RUSTFLAGS="--cfg loom" cargo test --test
//! loom_model` — under `--cfg loom` this module uses loom's mutex and nothing
//! else in the crate changes shape.

use std::collections::BTreeMap;

/// Identity of a node. Ids are handed out in creation order, which is also the
/// order effects and memos are flushed in.
pub(crate) type Id = u64;

/// The lock behind [`Lock`]. `loom::sync::Mutex` under `--cfg loom`, so the model
/// sees a real mutex; `RefCell` otherwise, because the graph is single-threaded
/// and pays nothing for it.
#[cfg(loom)]
type Raw<T> = loom::sync::Mutex<T>;
#[cfg(not(loom))]
type Raw<T> = std::cell::RefCell<T>;

/// A cell with one access shape on both backends.
struct Lock<T>(Raw<T>);

impl<T> Lock<T> {
    fn new(value: T) -> Self {
        #[cfg(loom)]
        {
            Lock(loom::sync::Mutex::new(value))
        }
        #[cfg(not(loom))]
        {
            Lock(std::cell::RefCell::new(value))
        }
    }

    /// Borrow the value for the duration of `f`. Loom poisons nothing, but its
    /// `lock()` returns a `LockResult` all the same.
    fn with<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
        #[cfg(loom)]
        {
            let mut guard = self.0.lock().unwrap_or_else(|error| error.into_inner());
            f(&mut guard)
        }
        #[cfg(not(loom))]
        {
            f(&mut self.0.borrow_mut())
        }
    }
}

/// A map from [`Id`] to a handle, safe to read and write from several threads
/// when it is built with loom's types.
pub(crate) struct Registry<H> {
    map: Lock<BTreeMap<Id, H>>,
}

impl<H> Registry<H> {
    pub(crate) fn new() -> Self {
        Registry {
            map: Lock::new(BTreeMap::new()),
        }
    }

    /// Bind `id` to `handle`, replacing any previous binding.
    pub(crate) fn insert(&self, id: Id, handle: H) {
        self.map.with(|map| map.insert(id, handle));
    }

    /// Unbind `id`, returning the handle it held.
    pub(crate) fn remove(&self, id: Id) -> Option<H> {
        self.map.with(|map| map.remove(&id))
    }

    /// The handle bound to `id`.
    pub(crate) fn get(&self, id: Id) -> Option<H>
    where
        H: Clone,
    {
        self.map.with(|map| map.get(&id).cloned())
    }

    /// Read the map directly. The caller must not re-enter this registry from
    /// inside `f`, and must not hold anything derived from the map past it —
    /// under loom that would be holding a lock across a call that may block.
    pub(crate) fn with<R>(&self, f: impl FnOnce(&BTreeMap<Id, H>) -> R) -> R {
        self.map.with(|map| f(map))
    }

    /// How many ids are bound.
    pub(crate) fn len(&self) -> usize {
        self.map.with(|map| map.len())
    }

    /// `true` when nothing is bound.
    pub(crate) fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Forget every binding.
    pub(crate) fn clear(&self) {
        self.map.with(|map| map.clear());
    }

    /// Every id currently bound, ascending.
    pub(crate) fn ids(&self) -> Vec<Id> {
        self.map.with(|map| map.keys().copied().collect())
    }
}

impl<H> Default for Registry<H> {
    fn default() -> Self {
        Registry::new()
    }
}

/// The run counter. Counters only move forward inside a flush, and a single
/// thread does that, so this one does not need to be generic.
impl Registry<u32> {
    /// One more run for `id`, returning the new count.
    pub(crate) fn bump(&self, id: Id) -> u32 {
        self.map.with(|map| {
            let count = map.entry(id).or_insert(0);
            *count += 1;
            *count
        })
    }
}