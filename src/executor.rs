//! A minimal single-threaded executor, so futures can drive signals without a
//! dependency on an async runtime.
//!
//! `std::task::Wake` demands a `Send + Sync` waker, which a single-threaded task
//! is not. So the waker payload is just a task id: waking pushes the id onto a
//! thread-local queue, and [`run_ready`] polls exactly those tasks. No `unsafe`,
//! and nothing to get wrong at the ABI level.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;
use std::task::{Context, Wake, Waker};

type BoxFuture = Pin<Box<dyn Future<Output = ()>>>;

struct Job {
    future: RefCell<Option<BoxFuture>>,
    /// `true` while the task sits in the ready queue, so one wake means one poll.
    queued: Cell<bool>,
    finished: Cell<bool>,
}

/// The waker payload: a plain id, which satisfies `Send + Sync` for free.
struct WakeById {
    id: u64,
}

impl Wake for WakeById {
    fn wake(self: Arc<Self>) {
        enqueue(self.id);
    }

    fn wake_by_ref(self: &Arc<Self>) {
        enqueue(self.id);
    }
}

thread_local! {
    static TASKS: RefCell<BTreeMap<u64, Rc<Job>>> = const { RefCell::new(BTreeMap::new()) };
    static READY: RefCell<VecDeque<u64>> = const { RefCell::new(VecDeque::new()) };
    static NEXT_ID: Cell<u64> = const { Cell::new(0) };
}

fn next_id() -> u64 {
    NEXT_ID.with(|n| {
        let id = n.get();
        n.set(id + 1);
        id
    })
}

/// Mark the task ready. Waking twice before a poll is one poll.
fn enqueue(id: u64) {
    let queued = TASKS.with(|tasks| {
        tasks
            .borrow()
            .get(&id)
            .map(|task| task.queued.replace(true))
            .unwrap_or(false)
    });
    if !queued {
        READY.with(|r| r.borrow_mut().push_back(id));
    }
}

fn take(id: u64) -> Option<Rc<Job>> {
    TASKS.with(|tasks| tasks.borrow_mut().remove(&id))
}

fn get(id: u64) -> Option<Rc<Job>> {
    TASKS.with(|tasks| tasks.borrow().get(&id).cloned())
}

/// A spawned future.
///
/// Dropping the handle cancels the future — unless it was spawned inside a scope
/// (a [`crate::Root`], [`crate::create_root`] run, or an effect), in which case
/// the scope owns it and it is cancelled when the scope is disposed.
#[derive(Debug)]
pub struct Task {
    id: u64,
    cancel_on_drop: bool,
}

impl Task {
    /// Stop the task now. Same as dropping the handle outside a scope.
    pub fn cancel(mut self) {
        self.cancel_on_drop = false;
        let _finished = take(self.id);
    }

    /// `true` once the future has completed or the task was cancelled.
    pub fn is_finished(&self) -> bool {
        get(self.id).is_none_or(|task| task.finished.get())
    }
}

impl Drop for Task {
    fn drop(&mut self) {
        if self.cancel_on_drop {
            let _finished = take(self.id);
        }
    }
}

/// Spawn a future on the local executor. It is polled at the next
/// [`crate::tick`], and again each time it wakes itself.
///
/// Inside a scope the future belongs to that scope, exactly like an effect: it
/// runs until the scope is disposed, even if the [`Task`] handle is dropped.
/// Outside a scope, dropping the handle cancels.
///
/// ```
/// use rok_ui_hooks::*;
///
/// let (text, set_text) = use_state(String::new());
/// let signal = set_text.clone();
/// let _task = spawn(async move {
///     signal.set("loaded".into());
/// });
///
/// tick(); // polls the task
/// assert_eq!(text.get(), "loaded");
/// ```
pub fn spawn<F: Future<Output = ()> + 'static>(future: F) -> Task {
    let id = next_id();
    TASKS.with(|tasks| {
        tasks.borrow_mut().insert(
            id,
            Rc::new(Job {
                future: RefCell::new(Some(Box::pin(future))),
                queued: Cell::new(false),
                finished: Cell::new(false),
            }),
        );
    });
    enqueue(id);

    // Owned by the enclosing scope, if there is one.
    let owned = crate::runtime::current_owner().is_some();
    if owned {
        crate::runtime::on_cleanup(move || {
            let _finished = take(id);
        });
    }

    Task {
        id,
        cancel_on_drop: !owned,
    }
}

/// How many times one task may be polled inside a single [`crate::tick`]. A
/// future that wakes itself on every poll — a generator, or a loop with a bug —
/// would otherwise spin forever inside one frame. Hitting the cap is not an
/// error: the task keeps its place in the queue and resumes on the next tick.
const MAX_POLLS_PER_TICK: u32 = 100;

/// Poll every ready task until none is left. Called by [`crate::tick`].
pub(crate) fn run_ready() {
    let mut polls: BTreeMap<u64, u32> = BTreeMap::new();
    while let Some(id) = READY.with(|r| r.borrow_mut().pop_front()) {
        let count = polls.entry(id).or_insert(0);
        *count += 1;
        if *count > MAX_POLLS_PER_TICK {
            // Give the rest of this tick back to the caller; this task waits for
            // the next one.
            enqueue(id);
            break;
        }
        poll(id);
    }
}

fn poll(id: u64) {
    let Some(task) = get(id) else {
        return;
    };
    task.queued.set(false);
    if task.finished.get() {
        let _unused = take(id);
        return;
    }

    let waker = Waker::from(Arc::new(WakeById { id }));
    let mut context = Context::from_waker(&waker);

    // No map borrow is held here, so a future is free to spawn more work.
    let mut slot = task.future.borrow_mut();
    let Some(future) = slot.as_mut() else {
        let _unused = take(id);
        return;
    };
    if future.as_mut().poll(&mut context).is_ready() {
        task.finished.set(true);
        *slot = None;
        drop(slot);
        let _unused = take(id);
    }
}

/// How many tasks are still alive. Useful in diagnostics and tests.
pub fn pending_tasks() -> usize {
    TASKS.with(|tasks| tasks.borrow().len())
}
