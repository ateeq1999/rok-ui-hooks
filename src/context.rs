//! A React-style Context API on top of signals.
//!
//! * [`create_context`] — make a typed context with a default value.
//! * [`with_provider`] — the `<Ctx.Provider value={…}>` equivalent: a scoped,
//!   panic-safe push of a fresh value cell.
//! * [`use_context`] — a **tracked** read, so consumers re-run when the value
//!   changes (React's "provider value changed → re-render consumers").
//!
//! A computation captures the providers that are visible where it is created and
//! re-installs them on every run, so `use_context` resolves by *creation site*,
//! not by whatever happens to be in scope when the effect re-runs.

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::signal::{ReadSignal, WriteSignal, create_signal};

pub(crate) type ContextId = u64;

thread_local! {
    static NEXT_CONTEXT_ID: Cell<ContextId> = const { Cell::new(0) };
    static PROVIDER_STACK: RefCell<Vec<ProviderFrame>> = const { RefCell::new(Vec::new()) };
}

/// One active provider: which context it serves, and the cell holding its value.
/// The cell is a `(ReadSignal<T>, WriteSignal<T>)` pair, type-erased here.
#[derive(Clone)]
pub(crate) struct ProviderFrame {
    pub(crate) context_id: ContextId,
    pub(crate) cell: Rc<dyn Any>,
}

type Cell2<T> = (ReadSignal<T>, WriteSignal<T>);

/// A typed context, comparable to React's `createContext(defaultValue)`.
pub struct Context<T> {
    id: ContextId,
    default: Cell2<T>,
}

impl<T> Clone for Context<T> {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            default: self.default.clone(),
        }
    }
}

/// `createContext(defaultValue)`.
pub fn create_context<T: 'static>(default: T) -> Context<T> {
    let id = NEXT_CONTEXT_ID.with(|n| {
        let id = n.get();
        n.set(id + 1);
        id
    });
    Context {
        id,
        default: create_signal(default),
    }
}

impl<T: 'static> Context<T> {
    /// Method form of [`with_provider`].
    pub fn provide<R>(&self, value: T, f: impl FnOnce() -> R) -> R {
        with_provider(self, value, f)
    }
}

impl<T: Clone + 'static> Context<T> {
    /// Tracked read of the nearest provider's value, or the default.
    pub fn get(&self) -> T {
        use_context(self)
    }

    /// Write the nearest provider's value (or the default when unprovided),
    /// notifying every consumer that read it.
    pub fn set(&self, value: T) {
        match resolve(self.id) {
            Some((_read, write)) => write.set(value),
            None => self.default.1.set(value),
        }
    }
}

/// `<Ctx.Provider value={value}>` — push a value for the dynamic extent of `f`.
pub fn with_provider<T: 'static, R>(ctx: &Context<T>, value: T, f: impl FnOnce() -> R) -> R {
    struct PopGuard;
    impl Drop for PopGuard {
        fn drop(&mut self) {
            PROVIDER_STACK.with(|s| {
                s.borrow_mut().pop();
            });
        }
    }

    let cell: Rc<Cell2<T>> = Rc::new(create_signal(value));
    let frame = ProviderFrame {
        context_id: ctx.id,
        cell: cell as Rc<dyn Any>,
    };
    PROVIDER_STACK.with(|s| s.borrow_mut().push(frame));
    let _guard = PopGuard;
    f()
}

/// `useContext(ctx)` — tracked read of the nearest provider value.
pub fn use_context<T: Clone + 'static>(ctx: &Context<T>) -> T {
    match resolve(ctx.id) {
        Some((read, _write)) => read.get(),
        None => ctx.default.0.get(),
    }
}

/// Nearest provider cell for `id`, innermost first.
fn resolve<T: 'static>(id: ContextId) -> Option<Cell2<T>> {
    PROVIDER_STACK.with(|s| {
        s.borrow()
            .iter()
            .rev()
            .find(|f| f.context_id == id)
            .and_then(|f| f.cell.downcast_ref::<Cell2<T>>().cloned())
    })
}

/// Snapshot the provider stack (called when a computation is created).
pub(crate) fn capture_providers() -> Vec<ProviderFrame> {
    PROVIDER_STACK.with(|s| s.borrow().clone())
}

/// Swap in a captured stack for the extent of `f`, then restore.
pub(crate) fn with_captured_providers<R>(frames: &[ProviderFrame], f: impl FnOnce() -> R) -> R {
    struct Restore(Vec<ProviderFrame>);
    impl Drop for Restore {
        fn drop(&mut self) {
            PROVIDER_STACK.with(|s| *s.borrow_mut() = std::mem::take(&mut self.0));
        }
    }

    let saved = PROVIDER_STACK.with(|s| std::mem::replace(&mut *s.borrow_mut(), frames.to_vec()));
    let _guard = Restore(saved);
    f()
}
