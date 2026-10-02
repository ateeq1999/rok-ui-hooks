//! Hooks that are state plus a policy: `use_reducer`, `use_previous`,
//! `use_debounced`, `use_throttled`, and the owned-scope primitive.

use std::rc::Rc;
use std::time::{Duration, Instant};

use crate::effect::use_ref;
use crate::effect::{Effect, use_effect};
use crate::memo::{Memo, use_memo};
use crate::signal::{ReadSignal, WriteSignal, use_state};
use crate::timer::{self, Timer};

type Reducer<S, A> = dyn Fn(&S, A) -> S;

/// `useReducer(reducer, initialState)` → `(state, dispatch)`.
///
/// Every update goes through one pure function, which is easier to reason about
/// (and to test) than scattered `set_state` calls.
///
/// ```
/// use rok_ui_hooks::*;
///
/// #[derive(Clone, PartialEq, Debug)]
/// enum Action { Inc, Dec }
///
/// let (count, dispatch) = use_reducer(0, |count: &i32, action: Action| match action {
///     Action::Inc => count + 1,
///     Action::Dec => count - 1,
/// });
///
/// dispatch.dispatch(Action::Inc);
/// dispatch.dispatch(Action::Dec);
/// dispatch.dispatch(Action::Dec);
/// assert_eq!(count.get(), -1);
/// ```
#[derive(Clone)]
pub struct Dispatch<S, A> {
    read: ReadSignal<S>,
    state: WriteSignal<S>,
    reducer: Rc<Reducer<S, A>>,
}

impl<S, A> std::fmt::Debug for Dispatch<S, A> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Dispatch")
    }
}

impl<S: 'static, A: 'static> Dispatch<S, A> {
    /// Apply an action.
    pub fn dispatch(&self, action: A) {
        let next = self
            .read
            .with_untracked(|current| (self.reducer)(current, action));
        self.state.set(next);
    }
}

/// `useReducer(reducer, initialState)`.
pub fn use_reducer<S, A>(
    initial: S,
    reducer: impl Fn(&S, A) -> S + 'static,
) -> (ReadSignal<S>, Dispatch<S, A>)
where
    S: 'static,
    A: 'static,
{
    let (read, set_state) = use_state(initial);
    let dispatch = Dispatch {
        read: read.clone(),
        state: set_state,
        reducer: Rc::new(reducer),
    };
    (read, dispatch)
}

/// `usePrevious(value)` — the value `source` had the last time this was read.
///
/// Read it from inside an effect: that read is what advances the history.
///
/// ```
/// use rok_ui_hooks::*;
///
/// let (n, set_n) = use_state(1);
/// let previous = use_previous(&n);
/// let _e = use_effect(
///     move || {
///         let now = n.get();
///         println!("{now} (was {:?})", previous.get());
///     },
///     (),
/// );
/// set_n.set(2);
/// set_n.set(3);
/// ```
pub fn use_previous<T: Clone + PartialEq + 'static>(source: &ReadSignal<T>) -> Memo<Option<T>> {
    let source = source.clone();
    let previous = use_ref(None::<T>);
    use_memo(
        move || {
            let next = source.get();
            previous.borrow_mut().replace(next)
        },
        (),
    )
}

/// A signal that only reports its source once the source stopped changing for a
/// quiet period.
///
/// The value lands on the next [`crate::tick`] after the deadline, so a UI loop
/// should call `tick()` once per frame.
pub struct Debounced<T> {
    read: ReadSignal<T>,
    _effect: Effect,
}

impl<T> std::fmt::Debug for Debounced<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Debounced").finish()
    }
}

impl<T: Clone + 'static> Debounced<T> {
    /// The settled value.
    pub fn get(&self) -> T {
        self.read.get()
    }

    /// Read the settled value by reference.
    pub fn with<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        self.read.with(f)
    }

    /// The underlying signal, for passing down the tree.
    pub fn read_signal(&self) -> ReadSignal<T> {
        self.read.clone()
    }
}

/// `useDebounced(source, quiet)` — the trailing edge of a burst of writes.
pub fn use_debounced<T: Clone + 'static>(source: &ReadSignal<T>, quiet: Duration) -> Debounced<T> {
    let (out, set_out) = use_state(source.get_untracked());
    let pending = use_ref(None::<T>);
    let timer = use_ref(None::<Timer>);
    // The mount run only reads the initial value, which `out` already holds.
    let mounted = use_ref(false);

    let effect = use_effect(
        {
            let (source, pending, timer, mounted) = (
                source.clone(),
                pending.clone(),
                timer.clone(),
                mounted.clone(),
            );
            move || {
                let value = source.get();
                if !mounted.replace(true) {
                    return;
                }

                *pending.borrow_mut() = Some(value);

                let slot = pending.clone();
                let sink = set_out.clone();
                let scheduled = timer::schedule(quiet, move || {
                    if let Some(latest) = slot.borrow_mut().take() {
                        sink.set(latest);
                    }
                });
                // Restart the clock: only the last write of a burst survives.
                if let Some(previous) = timer.borrow_mut().replace(scheduled) {
                    previous.cancel();
                }
            }
        },
        (),
    );

    Debounced {
        read: out,
        _effect: effect,
    }
}

/// A signal that reports at most once per window: the first change goes through
/// immediately, the rest are coalesced into one trailing update.
pub struct Throttled<T> {
    read: ReadSignal<T>,
    _effect: Effect,
}

impl<T> std::fmt::Debug for Throttled<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Throttled").finish()
    }
}

impl<T: Clone + 'static> Throttled<T> {
    /// The current throttled value.
    pub fn get(&self) -> T {
        self.read.get()
    }

    /// Read the current value by reference.
    pub fn with<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        self.read.with(f)
    }

    /// The underlying signal, for passing down the tree.
    pub fn read_signal(&self) -> ReadSignal<T> {
        self.read.clone()
    }
}

/// `useThrottled(source, window)` — leading edge plus one trailing update.
pub fn use_throttled<T: Clone + 'static>(source: &ReadSignal<T>, window: Duration) -> Throttled<T> {
    let (out, set_out) = use_state(source.get_untracked());
    let emitted_at = use_ref(None::<Instant>);
    // The mount run must not spend the leading edge, or the first real change
    // would be delayed by a whole window.
    let mounted = use_ref(false);
    // The pending trailing timer has to be kept alive: dropping a `Timer`
    // cancels it, and the whole point is that it outlives this run.
    let trailing = use_ref(None::<Timer>);

    let effect = use_effect(
        {
            let (source, emitted_at, mounted, trailing) = (
                source.clone(),
                emitted_at.clone(),
                mounted.clone(),
                trailing.clone(),
            );
            move || {
                let value = source.get();
                if !mounted.replace(true) {
                    return;
                }

                let now = Instant::now();
                let elapsed = emitted_at
                    .borrow()
                    .map(|at| now.saturating_duration_since(at));
                let leading = elapsed.is_none_or(|since| since >= window);

                if leading {
                    emitted_at.borrow_mut().replace(now);
                    set_out.set(value);
                } else {
                    let wait = window - elapsed.unwrap_or_default();
                    let sink = set_out.clone();
                    let stamp = emitted_at.clone();
                    let scheduled = timer::schedule(wait, move || {
                        stamp.borrow_mut().replace(Instant::now());
                        sink.set(value);
                    });
                    // One trailing update only: the last write of the burst wins.
                    if let Some(previous) = trailing.borrow_mut().replace(scheduled) {
                        previous.cancel();
                    }
                }
            }
        },
        (),
    );

    Throttled {
        read: out,
        _effect: effect,
    }
}

/// A scope that outlives the call that created it. Dropping it disposes
/// everything created inside.
///
/// Unlike [`crate::create_root`] this one is returned, which is what a container
/// needs: [`crate::create_keyed_list`] uses it to own one subtree per key.
///
/// ```
/// use rok_ui_hooks::*;
///
/// let (n, set_n) = use_state(0);
/// let ran = std::rc::Rc::new(std::cell::Cell::new(0));
///
/// let (scope, ()) = create_owned_root(|| {
///     let r = ran.clone();
///     let s = n.clone();
///     let _e = use_effect(move || {
///         s.get();
///         r.set(r.get() + 1);
///     }, ());
/// });
///
/// set_n.set(1);
/// assert_eq!(ran.get(), 2);
/// scope.dispose();
/// set_n.set(2);
/// assert_eq!(ran.get(), 2);
/// ```
#[must_use = "dropping the scope disposes everything created inside it"]
pub struct OwnedRoot {
    node: Rc<crate::runtime::Node>,
}

impl std::fmt::Debug for OwnedRoot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OwnedRoot")
            .field("id", &self.node.id)
            .finish()
    }
}

impl OwnedRoot {
    /// Dispose the scope and everything it owns.
    pub fn dispose(&self) {
        crate::runtime::dispose_node(self.node.id);
    }

    /// Run `f` inside this scope.
    pub fn run<R>(&self, f: impl FnOnce() -> R) -> R {
        crate::runtime::run_in_owner(self.node.id, f)
    }
}

impl Drop for OwnedRoot {
    fn drop(&mut self) {
        self.dispose();
    }
}

/// Create a long-lived owned scope. See [`OwnedRoot`].
pub fn create_owned_root<R>(f: impl FnOnce() -> R) -> (OwnedRoot, R) {
    let node = crate::runtime::Node::create_owner();
    let root = OwnedRoot { node };
    let value = root.run(f);
    (root, value)
}
