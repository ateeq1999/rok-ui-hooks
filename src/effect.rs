//! Effects — `use_effect`, `use_effect_with`, `use_cleanup`, `use_ref`.

use std::cell::RefCell;
use std::rc::Rc;

use crate::deps::{Deps, DepsBox};
use crate::runtime::{Computation, current_observer};

/// Owning handle to a running effect. Dropping it disposes the effect.
#[must_use = "dropping an Effect immediately disposes it"]
pub struct Effect {
    _computation: Rc<Computation>,
}

impl Effect {
    /// Stop the effect now (same as dropping it).
    pub fn dispose(self) {}

    /// Keep the effect alive for the rest of the program.
    pub fn leak(self) {
        std::mem::forget(self);
    }
}

/// `useEffect(fn, deps)` — run `f` now, and again when a dependency changes.
///
/// * `deps = ()` — no declared dependencies: re-run whenever any signal the body
///   read changes (automatic fine-grained tracking).
/// * `deps = (count,)` — re-run only when one of the listed **handles** has a
///   new value. A notification caused by something else leaves the effect alone,
///   exactly like React's `[deps]` array.
/// * `deps = (0i32,)` — constant: the effect runs on mount only (React `[]`).
///
/// The returned `Effect` must be kept alive — bind it to `_e`.
pub fn use_effect(f: impl FnMut() + 'static, deps: impl Deps + 'static) -> Effect {
    let computation = Computation::new(f, Some(Box::new(DepsBox(deps))));
    computation.run();
    Effect {
        _computation: computation,
    }
}

/// `useEffect(fn, deps)` where `fn` **returns a cleanup callback**.
///
/// The cleanup runs before the next run and when the effect is disposed. This is
/// why the two variants exist: Rust cannot have one function accept both `()` and
/// closures as the return value without coherence conflicts.
pub fn use_effect_with<C: FnOnce() + 'static>(
    mut f: impl FnMut() -> C + 'static,
    deps: impl Deps + 'static,
) -> Effect {
    let computation = Computation::new(
        move || {
            let cleanup = f();
            if let Some(running) = current_observer() {
                running.cleanups.borrow_mut().push(Box::new(cleanup));
            }
        },
        Some(Box::new(DepsBox(deps))),
    );
    computation.run();
    Effect {
        _computation: computation,
    }
}

/// `onCleanup(fn)` — register teardown for the effect or memo that is running.
/// It fires before that computation re-runs and when it is disposed.
pub fn use_cleanup(f: impl FnOnce() + 'static) {
    if let Some(running) = current_observer() {
        running.cleanups.borrow_mut().push(Box::new(f));
    }
}

/// `useRef(initial)` — a stable mutable box, handy for counting runs or holding
/// something you do not want to be a dependency.
pub fn use_ref<T>(initial: T) -> Rc<RefCell<T>> {
    Rc::new(RefCell::new(initial))
}
