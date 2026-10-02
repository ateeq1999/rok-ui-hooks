//! Fine-grained reactivity with a React-flavoured API.
//!
//! Components — if you write any — run **once**. Work is organised into effects,
//! which re-run when the signals they read change, and memos, which recompute
//! lazily when somebody reads them. No component tree is ever re-executed, so
//! adding state never costs a re-render.
//!
//! ```
//! use signals::*;
//!
//! let (count, set_count) = use_state(0);
//! let doubled = use_memo(
//!     { let count = count.clone(); move || count.get() * 2 },
//!     (),
//! );
//! let _effect = use_effect(
//!     {
//!         let (count, doubled) = (count.clone(), doubled.clone());
//!         move || println!("count = {}, doubled = {}", count.get(), doubled.get())
//!     },
//!     (doubled,),
//! );
//!
//! set_count.set(2); // count = 2, doubled = 4
//! ```
//!
//! # The mental model
//!
//! The graph has three kinds of node, and the difference between them is the
//! whole design:
//!
//! * **signals** hold a value. They have no dependencies. A write bumps a version
//!   and *marks* what reads them.
//! * **memos** are derived values. They are lazy: a marked memo recomputes when
//!   somebody reads it, and its readers only wake up if the value really changed.
//! * **effects** are side effects. They are eager: a marked effect is queued and
//!   runs when the enclosing batch ends.
//!
//! A write runs nothing synchronously, which is what makes any graph shape
//! glitch-free — see [`batch`], [`untrack`] and [`flush`] for the controls, and
//! [`Root`] for scope.
//!
//! # Mapping to React and Solid
//!
//! | React                            | This crate                                            |
//! |----------------------------------|-------------------------------------------------------|
//! | `useState(v)`                    | [`use_state`] → `(ReadSignal, WriteSignal)`   |
//! | `useEffect(fn, deps)`            | [`use_effect`] / [`use_effect_with`]                    |
//! | `useMemo(fn, deps)`              | [`use_memo`] → [`Memo`]                                 |
//! | `useReducer`                     | [`use_reducer`]                                        |
//! | `useRef(v)`                      | [`use_ref`] → `Rc<RefCell<T>>`                          |
//! | `useContext(ctx)`                | [`use_context`]                                        |
//! | `createContext` / `<Provider>`    | [`create_context`] / [`with_provider`]                 |
//! | Solid `createRoot`               | [`create_root`], [`Root`]                               |
//! | Solid `createEffect`             | [`create_effect`]                                      |
//! | Solid `createRenderEffect`       | [`create_render_effect`]                               |
//! | `create()` + `useStore(s, sel)`   | [`create_store`] + [`use_store`]                        |
//! | `createSelector`                 | [`Store::select`]                                      |
//! | `createResource`                 | [`use_resource`]                                       |
//! | `<For each={…}>`                 | [`create_keyed_list`]                                  |
//! | `onCleanup(fn)`                  | [`on_cleanup`], [`use_cleanup`]                        |
//!
//! # Dependencies
//!
//! A `deps` argument is a tuple of **handles** and/or plain values; it is
//! re-evaluated on every notification, so a handle contributes its current
//! *value*:
//!
//! | deps        | meaning                                                       |
//! |-------------|---------------------------------------------------------------|
//! | `()`        | no declared deps: re-run when anything the body read changes    |
//! | `(count,)`  | re-run only when `count` has a new value (React `[count]`)      |
//! | `(0i32,)`   | constant: mount only (React `[]`)                               |
//!
//! # Where to look next
//!
//! * `README.md` and `docs/getting-started.md` — the guided tour.
//! * The `examples/` directory: `demo`, `advanced`, `todo_app`, `ownership`,
//!   `keyed_list`, `async`, `context_demo`, `hooks_demo`, `store_demo`.
//! * `docs/concepts.md` for the model, `docs/api-reference.md` for every item,
//!   `docs/async-and-time.md` for the tick loop, `docs/patterns.md` for idioms
//!   and pitfalls, `docs/architecture.md` for the internals, and
//!   `docs/porting.md` for moving from React, Solid, or the 0.1 API.
//! * Every code block in the README and `docs/` is compiled as a doctest, so the
//!   documentation cannot drift from the code.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod context;
mod deps;
mod effect;
mod executor;
mod hooks;
mod keyed;
mod memo;
mod resource;
mod runtime;
mod signal;
mod store;
mod timer;

/// Compiles the code blocks in `README.md` as doctests. `cfg(doctest)` is set
/// by rustdoc only, so this costs nothing in a normal build.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
pub struct ReadmeDoctests;

/// Compiles `docs/*.md` the same way: documentation that does not build is worse
/// than no documentation.
#[cfg(doctest)]
mod markdown_doctests {
    #[doc = include_str!("../docs/getting-started.md")]
    pub struct GettingStarted;
    #[doc = include_str!("../docs/concepts.md")]
    pub struct Concepts;
    #[doc = include_str!("../docs/api-reference.md")]
    pub struct ApiReference;
    #[doc = include_str!("../docs/async-and-time.md")]
    pub struct AsyncAndTime;
    #[doc = include_str!("../docs/patterns.md")]
    pub struct Patterns;
    #[doc = include_str!("../docs/architecture.md")]
    pub struct Architecture;
    #[doc = include_str!("../docs/porting.md")]
    pub struct Porting;
}

pub use context::{Context, create_context, use_context, with_provider};
pub use deps::{DepItem, Deps};
pub use effect::{
    Effect, create_deferred_effect, create_effect, create_render_effect, use_cleanup, use_effect,
    use_effect_with, use_ref,
};
pub use executor::{Task, pending_tasks, spawn};
pub use hooks::{
    Debounced, Dispatch, OwnedRoot, Throttled, create_owned_root, use_debounced, use_previous,
    use_reducer, use_throttled,
};
pub use keyed::{KeyedList, create_keyed_list};
pub use memo::{Memo, create_memo, shallow_array_eq, shallow_vec_eq, use_memo, use_memo_eq};
pub use resource::{Resource, ResourceState, use_resource, use_resource_with};
pub use runtime::{Root, batch, create_root, flush, on_cleanup, tick, untrack};
pub use signal::{ReadSignal, WriteSignal, create_signal, use_state};
pub use store::{Store, Subscription, create_store, use_store};
