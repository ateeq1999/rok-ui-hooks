//! Fine-grained reactivity with a React-flavoured API: `use_state`,
//! `use_effect`, `use_memo`, a Context API and Zustand-style stores.
//!
//! The engine is fine-grained and single-threaded (`Rc`/`RefCell`, one reactive
//! world per thread). Components — if you write any — run **once**; work is
//! organised into effects, which re-run when the signals they read change. That
//! is the whole point: no component tree is ever re-executed, so scaling the
//! amount of state never costs a re-render.
//!
//! # Mapping to React
//!
//! | React                            | This crate                                              |
//! |----------------------------------|---------------------------------------------------------|
//! | `useState(v)`                    | `use_state(v) -> (ReadSignal<T>, WriteSignal<T>)`        |
//! | `useEffect(fn, deps)`            | `use_effect(fn, deps)` / `use_effect_with(fn, deps)`     |
//! | `useMemo(fn, deps)`              | `use_memo(fn, deps) -> Memo<T>`                          |
//! | `useRef(v)`                      | `use_ref(v) -> Rc<RefCell<T>>`                           |
//! | `createContext` / `<Provider>`    | `create_context` / `with_provider`                       |
//! | `useContext(ctx)`                | `use_context(ctx)`                                       |
//! | `create()` + `useStore(s, sel)`   | `create_store` + `use_store` / `Store::select`            |
//! | `store.subscribe(fn)`            | `Store::subscribe(fn) -> Subscription`                    |
//! | `onCleanup(fn)`                  | `use_cleanup(fn)`                                        |
//!
//! # Dependencies
//!
//! `deps` is a tuple of **handles** and/or plain values; it is re-evaluated on
//! every notification, so handles contribute their current *value*:
//!
//! * `()` — no declared deps: re-run when anything the body read changes.
//! * `(count,)` — re-run only when `count` has a new value (React `[count]`).
//! * `(0i32,)` — constant: mount only (React `[]`).
//!
//! # Quick tour
//!
//! ```
//! use signals::*;
//!
//! let (count, set_count) = use_state(0);
//! let doubled = use_memo(
//!     {
//!         let count = count.clone();
//!         move || count.get() * 2
//!     },
//!     (count.clone(),),
//! );
//!
//! let _effect = use_effect(
//!     {
//!         let (count, doubled) = (count.clone(), doubled.clone());
//!         move || println!("count = {}, doubled = {}", count.get(), doubled.get())
//!     },
//!     (doubled,),
//! );
//!
//! set_count.set(2);   // prints: count = 2, doubled = 4
//! ```

mod context;
mod deps;
mod effect;
mod memo;
mod runtime;
mod signal;
mod store;

pub use context::{Context, create_context, use_context, with_provider};
pub use deps::{DepItem, Deps};
pub use effect::{Effect, use_cleanup, use_effect, use_effect_with, use_ref};
pub use memo::{Memo, use_memo};
pub use runtime::{batch, untrack};
pub use signal::{ReadSignal, WriteSignal, use_state};
pub use store::{Store, Subscription, create_store, use_store};
