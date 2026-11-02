//! Real-time deadlines, without a dependency.
//!
//! A timer is just an `Instant` plus a callback, kept in a thread-local heap.
//! Nothing runs on its own thread: [`drain`] — called by [`crate::tick`] — pops
//! every entry whose deadline has passed and runs the callback **on the calling
//! thread**, so user closures never need to be `Send` and a timer can never touch
//! the graph from another thread.
//!
//! That is the whole safety story, and it has a price: a timer fires on the first
//! `tick()` *at or after* its deadline, so precision is bounded by how often the
//! owner ticks. [`crate::next_deadline`] is the other half of the answer — read it,
//! sleep until it, then tick — and [`crate::Driver`] does exactly that for you.
//!
//! [`schedule_after`] and [`schedule_at`] are public because the layers that sit
//! *above* a reactive core need a deadline they can own: a query with a stale
//! time, a form that validates later, a table that polls a page. Each of those
//! wants a handle it can cancel, and [`Timer`] is that handle.

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
///
/// A timer belongs to the thread that scheduled it, like everything else in the
/// crate: the callback runs inside [`crate::tick`], on the thread that calls it.
#[derive(Debug)]
pub struct Timer {
    token: u64,
    armed: Cell<bool>,
    /// Kept so [`Timer::reschedule_after`] can move the deadline without
    /// rebuilding the handle.
    deadline: Cell<Option<Instant>>,
}

impl Timer {
    /// Forget the timer: its callback will never run.
    pub fn cancel(&self) {
        if self.armed.replace(false) {
            PENDING.with(|p| {
                p.borrow_mut().remove(&self.token);
            });
            self.deadline.set(None);
        }
    }

    /// `true` until the timer fires or is cancelled.
    pub fn is_armed(&self) -> bool {
        self.armed.get()
    }

    /// When this timer is due, if it is still armed.
    pub fn deadline(&self) -> Option<Instant> {
        self.armed.get().then(|| self.deadline.get()).flatten()
    }

    /// Push the deadline back by `delay`, measured from now. The callback is the
    /// same one, so a poll that keeps missing its deadline can wait longer
    /// without giving up.
    ///
    /// ```
    /// use rok_ui_hooks::*;
    /// use std::time::Duration;
    ///
    /// let fired = std::rc::Rc::new(std::cell::Cell::new(false));
    /// let mut timer = schedule_after(Duration::from_secs(10), {
    ///     let fired = fired.clone();
    ///     move || fired.set(true)
    /// });
    /// timer.reschedule_after(Duration::from_millis(1));
    /// std::thread::sleep(Duration::from_millis(20));
    /// tick();
    /// assert!(fired.get());
    /// ```
    pub fn reschedule_after(&self, delay: Duration) {
        if self.armed.get() {
            self.deadline.set(Some(Instant::now() + delay));
            TIMERS.with(|timers| {
                let mut timers = timers.borrow_mut();
                if let Some(at) = self.deadline.get() {
                    timers.push(Reverse((at, self.token)));
                }
            });
        }
    }

    /// Move the deadline to `at`. A deadline in the past fires on the next tick.
    pub fn reschedule_at(&self, at: Instant) {
        if self.armed.get() {
            self.deadline.set(Some(at));
            TIMERS.with(|timers| {
                let mut timers = timers.borrow_mut();
                timers.push(Reverse((at, self.token)));
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
///
/// Dropping the returned [`Timer`] cancels it; so does the scope it was created
/// in, when the handle is owned by one.
pub fn schedule_after(delay: Duration, callback: impl FnOnce() + 'static) -> Timer {
    schedule_at(Instant::now() + delay, callback)
}

/// Run `callback` once `at` has passed, on the next [`crate::tick`] at or after
/// that instant.
///
/// A deadline in the past is not an error: it fires on the very next tick.
pub fn schedule_at(at: Instant, callback: impl FnOnce() + 'static) -> Timer {
    let token = NEXT_TOKEN.with(|n| {
        let token = n.get();
        n.set(token + 1);
        token
    });
    let slot: Callback = Rc::new(RefCell::new(Some(Box::new(callback))));
    PENDING.with(|p| p.borrow_mut().insert(token, slot));
    TIMERS.with(|t| {
        t.borrow_mut().push(Reverse((at, token)));
    });
    crate::scheduler::deadline_changed();
    Timer {
        token,
        armed: Cell::new(true),
        deadline: Cell::new(Some(at)),
    }
}

/// How many timers are waiting to fire. Useful in diagnostics and tests.
pub fn pending_timers() -> usize {
    PENDING.with(|p| p.borrow().len())
}

/// The earliest armed deadline, or `None` when nothing is scheduled.
///
/// This is what a loop without [`crate::Driver`] needs: sleep until this instant,
/// then [`crate::tick`].
pub fn next_deadline() -> Option<Instant> {
    let mut earliest = None;
    TIMERS.with(|timers| {
        // The heap is ordered earliest first, so the first entry that still owns
        // a callback is the answer. Cancelled timers keep their heap entry until
        // `drain` drops it, so skipping them is the whole job here.
        for Reverse((at, token)) in timers.borrow().iter() {
            let armed = PENDING.with(|p| p.borrow().contains_key(token));
            if armed {
                earliest = Some(*at);
                break;
            }
        }
    });
    earliest
}

/// Run every callback whose deadline has passed. Called by [`crate::tick`].
pub(crate) fn drain() -> usize {
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

    let mut fired = 0;
    for token in due {
        let Some(slot) = PENDING.with(|p| p.borrow_mut().remove(&token)) else {
            continue; // cancelled before it fired
        };
        let callback = slot.borrow_mut().take();
        if let Some(callback) = callback {
            callback();
            fired += 1;
        }
    }
    fired
}