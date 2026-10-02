//! Real-time deadlines for debounce and throttle, without a dependency.
//!
//! A timer is just an `Instant` plus a callback, kept in a thread-local heap.
//! Nothing runs on its own thread: [`drain`] — called by [`crate::tick`] — pops
//! every entry whose deadline has passed and runs the callback **on the calling
//! thread**, so user closures never need to be `Send` and a timer can never
//! touch the graph from another thread.
//!
//! The consequence is the one you want in a UI loop anyway: a timer fires on the
//! first `tick()` at or after its deadline, which keeps every graph mutation on
//! the thread that owns the graph. Outside a loop, call [`crate::tick`] on a
//! timer thread of your own, or use [`crate::flush`] plus a sleep.

use std::cell::{Cell, RefCell};
use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap};
use std::rc::Rc;
use std::time::{Duration, Instant};

/// `(deadline, token)`, earliest first.
type Deadline = Reverse<(Instant, u64)>;

/// A callback waiting for its deadline.
type Callback = Rc<RefCell<Option<Box<dyn FnOnce()>>>>;

thread_local! {
    /// Pending deadlines, earliest first.
    static TIMERS: RefCell<BinaryHeap<Deadline>> = const { RefCell::new(BinaryHeap::new()) };
    /// Callbacks by token. A cancelled timer keeps its heap entry, which
    /// [`drain`] simply drops.
    static PENDING: RefCell<BTreeMap<u64, Callback>> = const { RefCell::new(BTreeMap::new()) };
    static NEXT_TOKEN: Cell<u64> = const { Cell::new(0) };
}

/// Handle to a pending timer. Dropping it cancels.
#[derive(Debug)]
pub(crate) struct Timer {
    token: u64,
    armed: Cell<bool>,
}

impl Timer {
    /// Forget the timer: its callback will never run.
    pub(crate) fn cancel(&self) {
        if self.armed.replace(false) {
            PENDING.with(|p| {
                p.borrow_mut().remove(&self.token);
            });
        }
    }
}

impl Drop for Timer {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// Run `callback` once `delay` has passed, on the next [`crate::tick`] at or
/// after the deadline.
pub(crate) fn schedule(delay: Duration, callback: impl FnOnce() + 'static) -> Timer {
    let token = NEXT_TOKEN.with(|n| {
        let token = n.get();
        n.set(token + 1);
        token
    });
    let slot: Callback = Rc::new(RefCell::new(Some(Box::new(callback))));
    PENDING.with(|p| p.borrow_mut().insert(token, slot));
    TIMERS.with(|t| {
        t.borrow_mut()
            .push(Reverse((Instant::now() + delay, token)))
    });
    Timer {
        token,
        armed: Cell::new(true),
    }
}

/// Run every callback whose deadline has passed. Called by [`crate::tick`].
pub(crate) fn drain() {
    let now = Instant::now();
    let mut due = Vec::new();
    TIMERS.with(|timers| {
        let mut timers = timers.borrow_mut();
        while let Some(Reverse((at, _))) = timers.peek() {
            if *at > now {
                break;
            }
            let Reverse((_, token)) = timers.pop().expect("peeked");
            due.push(token);
        }
    });

    for token in due {
        let Some(slot) = PENDING.with(|p| p.borrow_mut().remove(&token)) else {
            continue; // cancelled before it fired
        };
        let callback = slot.borrow_mut().take();
        if let Some(callback) = callback {
            callback();
        }
    }
}
