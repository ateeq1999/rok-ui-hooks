//! The reactive engine: observer stack, computations, scheduling and batching.
//!
//! Nothing here is part of the public API except [`batch`] and [`untrack`].

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::{Rc, Weak};

use crate::context::{ProviderFrame, capture_providers, with_captured_providers};
use crate::deps::DepsAny;

pub(crate) type ComputationId = u64;

thread_local! {
    /// The observer context stack. `None` entries mean "untracked" (see [`untrack`]).
    static OBSERVER: RefCell<Vec<Option<Rc<Computation>>>> = const { RefCell::new(Vec::new()) };
    static NEXT_ID: Cell<ComputationId> = const { Cell::new(0) };
    static BATCH_DEPTH: Cell<usize> = const { Cell::new(0) };
    /// Computations waiting to run, keyed by id (= creation order).
    static PENDING: RefCell<BTreeMap<ComputationId, Weak<Computation>>> =
        const { RefCell::new(BTreeMap::new()) };
}

pub(crate) fn next_id() -> ComputationId {
    NEXT_ID.with(|n| {
        let id = n.get();
        n.set(id + 1);
        id
    })
}

pub(crate) fn current_observer() -> Option<Rc<Computation>> {
    OBSERVER.with(|o| o.borrow().last().cloned().flatten())
}

/// Push `obs` on the context stack, run `f`, and pop again — even if `f` panics.
fn with_observer<R>(obs: Option<Rc<Computation>>, f: impl FnOnce() -> R) -> R {
    struct PopGuard;
    impl Drop for PopGuard {
        fn drop(&mut self) {
            OBSERVER.with(|o| {
                o.borrow_mut().pop();
            });
        }
    }
    OBSERVER.with(|o| o.borrow_mut().push(obs));
    let _guard = PopGuard;
    f()
}

/// Type-erased view of a signal so a computation can unsubscribe from signals of any `T`.
pub(crate) trait Source {
    fn unsubscribe(&self, id: ComputationId);
}

/// The "running" object: a function plus everything it read and registered.
pub(crate) struct Computation {
    pub(crate) id: ComputationId,
    f: RefCell<Box<dyn FnMut()>>,
    /// Back-links: every signal read during the last run.
    pub(crate) sources: RefCell<Vec<Rc<dyn Source>>>,
    /// Callbacks registered with `use_cleanup` / returned by `use_effect_with`.
    pub(crate) cleanups: RefCell<Vec<Box<dyn FnOnce()>>>,
    deps: Option<Box<dyn DepsAny>>,
    prev_deps: RefCell<Option<Box<dyn Any>>>,
    /// Context providers visible where this computation was created. Re-installed
    /// on every run so a computation keeps the context of its creation site
    /// (React's "position in the tree", not dynamic scoping).
    providers: Vec<ProviderFrame>,
}

impl Computation {
    pub(crate) fn new(f: impl FnMut() + 'static, deps: Option<Box<dyn DepsAny>>) -> Rc<Self> {
        Rc::new(Computation {
            id: next_id(),
            f: RefCell::new(Box::new(f)),
            sources: RefCell::new(Vec::new()),
            cleanups: RefCell::new(Vec::new()),
            deps,
            prev_deps: RefCell::new(None),
            providers: capture_providers(),
        })
    }

    /// Cleanup → deps gate → push context → run.
    pub(crate) fn run(self: &Rc<Self>) {
        // If `f` is already borrowed, this computation is currently running
        // (it wrote to a signal it reads). Skip to avoid infinite recursion.
        let Ok(mut f) = self.f.try_borrow_mut() else {
            return;
        };

        if let Some(deps) = &self.deps {
            // Untracked: the comparison must not subscribe anybody.
            let next = untrack(|| deps.eval());
            let open = match &*self.prev_deps.borrow() {
                None => true,
                Some(prev) => deps.should_run(Some(prev.as_ref()), next.as_ref()),
            };
            if !open {
                return;
            }
            *self.prev_deps.borrow_mut() = Some(next);
        }

        self.cleanup();
        with_captured_providers(&self.providers, || {
            with_observer(Some(self.clone()), || {
                if let Some(deps) = &self.deps {
                    // Tracked: re-subscribe (cleanup just wiped the old links).
                    deps.eval();
                }
                f();
            })
        });
    }

    /// Unsubscribe from every source and run user cleanup callbacks.
    fn cleanup(&self) {
        let sources = std::mem::take(&mut *self.sources.borrow_mut());
        for source in sources {
            source.unsubscribe(self.id);
        }
        let cleanups = std::mem::take(&mut *self.cleanups.borrow_mut());
        for c in cleanups {
            c();
        }
    }

    fn is_running(&self) -> bool {
        self.f.try_borrow_mut().is_err()
    }

    /// Run now, or queue if we are inside a batch.
    pub(crate) fn schedule(self: &Rc<Self>) {
        if self.is_running() {
            return;
        }
        if BATCH_DEPTH.with(|d| d.get()) > 0 {
            // Keyed by id → duplicates collapse, and the queue stays sorted.
            PENDING.with(|p| {
                p.borrow_mut().insert(self.id, Rc::downgrade(self));
            });
        } else {
            self.run();
        }
    }
}

impl Drop for Computation {
    fn drop(&mut self) {
        self.cleanup();
    }
}

/// Run `f` without subscribing the current observer to anything it reads.
pub fn untrack<R>(f: impl FnOnce() -> R) -> R {
    with_observer(None, f)
}

/// Group writes: effects are queued and each one runs at most once per flush.
pub fn batch<R>(f: impl FnOnce() -> R) -> R {
    BATCH_DEPTH.with(|d| d.set(d.get() + 1));
    let result = f();

    // Only the outermost batch flushes. We stay at depth 1 while flushing so
    // writes made *by* effects are queued too, instead of running re-entrantly.
    // Always run the OLDEST pending computation next: a memo is created before
    // anything that reads it, so it gets refreshed before its readers run.
    if BATCH_DEPTH.with(|d| d.get()) == 1 {
        while let Some((_, weak)) = PENDING.with(|p| p.borrow_mut().pop_first()) {
            if let Some(c) = weak.upgrade() {
                c.run();
            }
        }
    }

    BATCH_DEPTH.with(|d| d.set(d.get() - 1));
    result
}
