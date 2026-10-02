//! Memos — cached derived values, `useMemo(fn, deps)`.

use std::cell::RefCell;
use std::rc::Rc;

use crate::deps::Deps;
use crate::effect::{Effect, use_effect};
use crate::signal::{ReadSignal, WriteSignal, create_signal};

/// A cached derived value. Trackable, like a signal: read it with [`Memo::get`]
/// or [`Memo::with`], and pass it inside another deps tuple to depend on it.
pub struct Memo<T> {
    read: ReadSignal<T>,
    _effect: Rc<Effect>,
}

impl<T> Clone for Memo<T> {
    fn clone(&self) -> Self {
        Self {
            read: self.read.clone(),
            _effect: self._effect.clone(),
        }
    }
}

/// `useMemo(fn, deps)` — recompute `f` when a dependency changes, and notify
/// readers only when the produced value actually differs (`T: PartialEq`).
///
/// Pass `()` for the automatic fine-grained behaviour, or a tuple of handles for
/// React-style declared dependencies — see [`Deps`].
pub fn use_memo<T: PartialEq + 'static>(
    mut f: impl FnMut() -> T + 'static,
    deps: impl Deps + 'static,
) -> Memo<T> {
    type Slot<T> = Rc<RefCell<Option<(ReadSignal<T>, WriteSignal<T>)>>>;
    let slot: Slot<T> = Rc::new(RefCell::new(None));

    let slot_in_effect = slot.clone();
    let effect = use_effect(
        move || {
            let next = f();
            // Clone out, then write in: never hold the borrow across a notify.
            let existing = slot_in_effect.borrow().clone();
            match existing {
                None => *slot_in_effect.borrow_mut() = Some(create_signal(next)),
                Some((read, write)) => {
                    // Untracked comparison, or the memo would subscribe to its
                    // own output and re-run whenever it writes itself.
                    if read.with_untracked(|old| *old != next) {
                        write.set(next);
                    }
                }
            }
        },
        deps,
    );

    let (read, _write) = slot.borrow().clone().expect("memo ran once on creation");
    Memo {
        read,
        _effect: Rc::new(effect),
    }
}

impl<T: 'static> Memo<T> {
    pub fn with<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        self.read.with(f)
    }

    pub fn get(&self) -> T
    where
        T: Clone,
    {
        self.read.get()
    }

    pub fn get_untracked(&self) -> T
    where
        T: Clone,
    {
        self.read.get_untracked()
    }
}
