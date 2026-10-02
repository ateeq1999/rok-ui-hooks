//! Dependency declarations for `use_effect` / `use_memo`.
//!
//! A deps argument is a tuple whose elements are either **handles**
//! ([`ReadSignal`](crate::ReadSignal) / [`Memo`](crate::Memo)) or **plain values**.
//! The library re-evaluates the tuple on every notification, so a handle element
//! turns into the signal's *current value*:
//!
//! | deps        | meaning                                                        |
//! |-------------|----------------------------------------------------------------|
//! | `()`        | no declared deps — automatic fine-grained tracking             |
//! | `(count,)`  | re-run when `count`'s **value** changes (React `[count]`)      |
//! | `(0i32,)`   | constant — run on mount only (React `[]`)                      |
//! | `(a, b, 0)` | mixed handles and constants                                    |

use std::any::Any;

use crate::memo::Memo;
use crate::signal::ReadSignal;

/// One element of a deps tuple.
///
/// Implemented for every `T: Clone + PartialEq` (a constant) and for handles
/// ([`ReadSignal`], [`Memo`]) where the *current value* is read instead.
pub trait DepItem {
    /// The value this element contributes to the comparison.
    type Out: PartialEq + 'static;
    /// Read the element's current value.
    fn current(&self) -> Self::Out;
}

impl<T: Clone + PartialEq + 'static> DepItem for T {
    type Out = T;
    fn current(&self) -> T {
        self.clone()
    }
}

impl<T: Clone + PartialEq + 'static> DepItem for ReadSignal<T> {
    type Out = T;
    fn current(&self) -> T {
        self.get()
    }
}

impl<T: Clone + PartialEq + 'static> DepItem for Memo<T> {
    type Out = T;
    fn current(&self) -> T {
        self.get()
    }
}

/// A whole deps tuple, evaluated and compared as a unit.
pub trait Deps {
    /// The tuple of current values produced by [`Deps::eval`].
    type Out: PartialEq + 'static;
    /// Re-evaluate the deps. Called inside the observer context (tracked) and
    /// outside it (untracked comparison), so context alone decides whether the
    /// reads subscribe.
    fn eval(&self) -> Self::Out;
    /// `true` for `()`: the gate is always open, i.e. plain automatic tracking.
    fn is_auto_track() -> bool
    where
        Self: Sized,
    {
        false
    }
}

impl Deps for () {
    type Out = ();
    fn eval(&self) {}

    fn is_auto_track() -> bool {
        true
    }
}

macro_rules! impl_deps {
    ($(($ty:ident, $idx:tt)),+ $(,)?) => {
        impl<$($ty: DepItem),+> Deps for ($($ty,)+) {
            type Out = ($(<$ty as DepItem>::Out,)+);
            fn eval(&self) -> Self::Out {
                ($(<$ty as DepItem>::current(&self.$idx),)+)
            }
        }
    };
}

impl_deps!((A, 0));
impl_deps!((A, 0), (B, 1));
impl_deps!((A, 0), (B, 1), (C, 2));
impl_deps!((A, 0), (B, 1), (C, 2), (D, 3));
impl_deps!((A, 0), (B, 1), (C, 2), (D, 3), (E, 4));
impl_deps!((A, 0), (B, 1), (C, 2), (D, 3), (E, 4), (F, 5));
impl_deps!((A, 0), (B, 1), (C, 2), (D, 3), (E, 4), (F, 5), (G, 6));
impl_deps!(
    (A, 0),
    (B, 1),
    (C, 2),
    (D, 3),
    (E, 4),
    (F, 5),
    (G, 6),
    (H, 7)
);

/// Type-erased [`Deps`], so non-generic `Computation`s can hold one.
pub(crate) trait DepsAny {
    fn eval(&self) -> Box<dyn Any>;
    /// `prev` is `None` on the very first (mount) run.
    fn should_run(&self, prev: Option<&dyn Any>, next: &dyn Any) -> bool;
}

pub(crate) struct DepsBox<D>(pub D);

impl<D: Deps> DepsAny for DepsBox<D> {
    fn eval(&self) -> Box<dyn Any> {
        Box::new(self.0.eval())
    }

    fn should_run(&self, prev: Option<&dyn Any>, next: &dyn Any) -> bool {
        if D::is_auto_track() {
            return true;
        }
        let Some(prev) = prev else { return true };
        match (
            prev.downcast_ref::<<D as Deps>::Out>(),
            next.downcast_ref::<<D as Deps>::Out>(),
        ) {
            (Some(p), Some(n)) => p != n,
            _ => true,
        }
    }
}
