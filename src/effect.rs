//! Effects — `use_effect`, `use_effect_with`, `use_cleanup`, `use_ref`, and the
//! `create_*` / deferred variants.

use std::cell::RefCell;
use std::rc::Rc;

use crate::deps::{Deps, DepsBox};
use crate::runtime::{Mode, Node, current_observer};

/// Owning handle to a running effect. Dropping it disposes the effect.
#[must_use = "dropping an Effect immediately disposes it"]
pub struct Effect {
    node: Rc<Node>,
}

impl Effect {
    /// Stop the effect now (same as dropping it).
    pub fn dispose(self) {}

    /// Keep the effect alive for the rest of the program.
    pub fn leak(self) {
        std::mem::forget(self);
    }

    /// `false` once the effect has been dropped, disposed or torn down with its
    /// owner.
    pub fn is_alive(&self) -> bool {
        !self.node.is_disposed()
    }
}

impl std::fmt::Debug for Effect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Effect").field("id", &self.node.id).finish()
    }
}

fn spawn_effect(f: impl FnMut() + 'static, deps: impl Deps + 'static, mode: Mode) -> Effect {
    let node = Node::create(true);
    node.set_mode(mode);
    node.set_deps(Rc::new(DepsBox(deps)));
    node.set_body(Box::new(f));
    Node::mount(&node);
    Effect { node }
}

/// `useEffect(fn, deps)` — run `f` now, and again when a dependency changes.
///
/// * `deps = ()` — automatic fine-grained tracking: re-run whenever a signal the
///   body read changes.
/// * `deps = (count,)` — re-run only when one of the listed **handles** has a
///   new value, exactly like React's `[deps]` array.
/// * `deps = (0i32,)` — constant: the effect runs on mount only (React `[]`).
///
/// The returned `Effect` must be kept alive — bind it to `_e`.
///
/// ```
/// use rok_ui_hooks::*;
///
/// let (count, set_count) = use_state(0);
/// let _e = use_effect(
///     { let count = count.clone(); move || println!("count = {}", count.get()) },
///     (),
/// );
/// set_count.set(1);
/// ```
pub fn use_effect(f: impl FnMut() + 'static, deps: impl Deps + 'static) -> Effect {
    spawn_effect(f, deps, Mode::Eager)
}

/// `createEffect(fn, deps)` — the Solid-flavoured name of [`use_effect`].
pub fn create_effect(f: impl FnMut() + 'static, deps: impl Deps + 'static) -> Effect {
    use_effect(f, deps)
}

/// Like [`use_effect`], but re-runs wait for an explicit [`crate::flush`].
///
/// This is your render frame: the body runs once on mount, then only when you
/// ask the runtime to paint. Pair it with [`crate::tick`] in a UI loop.
///
/// ```
/// use rok_ui_hooks::*;
///
/// let (n, set_n) = use_state(0);
/// let _e = create_deferred_effect(
///     { let n = n.clone(); move || println!("painting {}", n.get()) },
///     (),
/// );
/// set_n.set(1); // queued
/// flush();      // "painting 1"
/// ```
pub fn create_deferred_effect(f: impl FnMut() + 'static, deps: impl Deps + 'static) -> Effect {
    spawn_effect(f, deps, Mode::Deferred)
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
    use_effect(
        move || {
            let cleanup = f();
            if let Some(running) = current_observer() {
                running.cleanups.borrow_mut().push(Box::new(cleanup));
            }
        },
        deps,
    )
}

/// `useEffect(fn, deps)` where `fn` **returns** a cleanup callback, deferred to
/// the next frame. The `create_render_effect` counterpart of [`use_effect_with`].
pub fn create_render_effect<C: FnOnce() + 'static>(
    mut f: impl FnMut() -> C + 'static,
    deps: impl Deps + 'static,
) -> Effect {
    create_deferred_effect(
        move || {
            let cleanup = f();
            if let Some(running) = current_observer() {
                running.cleanups.borrow_mut().push(Box::new(cleanup));
            }
        },
        deps,
    )
}

/// `onCleanup(fn)` — register teardown for the effect, memo or owner that is
/// running. It fires before that computation re-runs and when it is disposed.
///
/// ```
/// use rok_ui_hooks::*;
///
/// let (n, set_n) = use_state(0);
/// let _e = use_effect(
///     move || {
///         println!("run {}", n.get());
///         use_cleanup(|| println!("cleanup"));
///     },
///     (),
/// );
/// set_n.set(1);
/// ```
pub fn use_cleanup(f: impl FnOnce() + 'static) {
    crate::runtime::on_cleanup(f);
}

/// `useRef(initial)` — a stable mutable box, handy for counting runs or holding
/// something you do not want to be a dependency.
///
/// ```
/// use rok_ui_hooks::*;
///
/// let (n, set_n) = use_state(0);
/// let runs = use_ref(0);
/// let counter = runs.clone();
/// let _e = use_effect(
///     move || {
///         n.get();
///         *counter.borrow_mut() += 1;
///     },
///     (),
/// );
/// set_n.set(1);
/// assert_eq!(*runs.borrow(), 2);
/// ```
pub fn use_ref<T>(initial: T) -> Rc<RefCell<T>> {
    Rc::new(RefCell::new(initial))
}
