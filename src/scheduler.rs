//! The frame loop, and the thread that knows when the next frame is due.
//!
//! A reactive graph does not need a loop of its own — writes flush by themselves
//! — but *time* does. A timer, and a future that is waiting to be polled, only
//! move when somebody calls [`crate::tick`]. Somebody has to decide when that is,
//! and that decision is this module.
//!
//! Three pieces, in increasing order of how much they take over:
//!
//! | what | who wakes the owner |
//! |------|---------------------|
//! | [`crate::tick`] | you, whenever you like |
//! | [`tick_within`] | a sleep inside the call, bounded by a budget you set |
//! | [`Scheduler`] + [`Driver`] | a background thread, so nothing is missed |
//!
//! # Why a driver thread is not the runtime
//!
//! A driver *notifies*. It never touches the graph: every callback still runs on
//! the thread that owns the graph, because user closures are not `Send` and
//! because a node is `Rc`-backed by design. That is why the answer to "a timer
//! fired while you were blocked" is `unpark`, not "run this on my thread".
//!
//! [`Driver`] exists for exactly one reason: precision. Without it a timer with a
//! 5 ms deadline fires on the first tick *after* that deadline, so precision is
//! bounded by your polling rate. With it, the driver waits on the earliest armed
//! deadline and unparks the owner the moment it passes.

use std::cell::RefCell;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::thread;
use std::time::{Duration, Instant};

/// What one frame did.
///
/// Zero in every field means the frame was idle: nothing was due, so a loop may
/// sleep without missing anything.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tick {
    /// Timer callbacks that fired.
    pub timers: usize,
    /// Futures that were polled.
    pub tasks: usize,
    /// Effects that ran, eager and deferred.
    pub effects: usize,
}

impl Tick {
    /// `true` when the frame had nothing to do.
    pub fn is_idle(&self) -> bool {
        self.timers == 0 && self.tasks == 0 && self.effects == 0
    }
}

/// Advance the world by one frame: fire due timers, poll woken tasks, then run
/// every pending effect. Returns what the frame did.
pub(crate) fn step() -> Tick {
    #[cfg(feature = "async")]
    let tasks = crate::executor::run_ready();
    #[cfg(not(feature = "async"))]
    let tasks = 0;
    Tick {
        timers: crate::timer::drain(),
        tasks,
        effects: crate::runtime::flush_count(),
    }
}

/// Advance the world, then wait for the next deadline — but never longer than
/// `max_wait`.
///
/// Returns as soon as something actually happened, so a caller that only wants to
/// service the graph cheaply can check [`Tick::is_idle`]. When there is nothing to
/// wait for it returns immediately.
///
/// ```
/// use rok_ui_hooks::*;
/// use std::time::Duration;
///
/// let (n, set_n) = use_state(0);
/// let _e = use_effect({ let n = n.clone(); move || n.get() }, ());
/// let fired = schedule_after(Duration::from_millis(20), move || set_n.set(1));
///
/// let first = tick_within(Duration::from_millis(5));
/// assert!(first.is_idle(), "nothing was due yet");
///
/// let second = tick_within(Duration::from_secs(1));
/// assert_eq!(second.timers, 1);
/// assert_eq!(n.get(), 1);
/// drop(fired);
/// ```
///
/// [`Tick::is_idle`]: Tick::is_idle
pub fn tick_within(max_wait: Duration) -> Tick {
    let mut tick = Tick::default();
    loop {
        let frame = step();
        tick.timers += frame.timers;
        tick.tasks += frame.tasks;
        tick.effects += frame.effects;
        if !tick.is_idle() {
            return tick;
        }
        match next_wakeup() {
            Some(deadline) => {
                let wait = deadline.saturating_duration_since(Instant::now());
                if wait > max_wait {
                    return tick;
                }
                thread::sleep(wait);
            }
            None => return tick,
        }
    }
}

/// The earliest moment anything needs this thread: an armed timer, or a task that
/// is already ready to poll.
///
/// `None` means the graph is quiet — no timer, nothing woken — so a hand-written
/// loop can block on something else until it knows there is work.
pub fn next_wakeup() -> Option<Instant> {
    let timer = crate::timer::next_deadline();
    #[cfg(feature = "async")]
    let task = crate::executor::next_ready();
    #[cfg(not(feature = "async"))]
    let task = None;

    match (timer, task) {
        (Some(timer), Some(task)) => Some(timer.min(task)),
        (timer, task) => timer.or(task),
    }
}

/// A loop the crate drives instead of you.
///
/// ```
/// use rok_ui_hooks::*;
/// use std::time::Duration;
///
/// let (count, set_count) = use_state(0);
/// let _e = use_effect(
///     { let count = count.clone(); move || println!("frame: {}", count.get()) },
///     (),
/// );
///
/// let mut scheduler = Scheduler::new();
/// // Stop after the second frame that actually did something.
/// scheduler.run_until(|tick| tick.effects >= 2);
/// set_count.set(1);
/// ```
pub struct Scheduler {
    /// The floor on how long an idle step may sleep. Not a ceiling: a
    /// [`Driver`] unparks the owner exactly on the deadline, so this only stops a
    /// future that re-wakes itself from spinning the loop.
    floor: Duration,
}

impl std::fmt::Debug for Scheduler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Scheduler")
            .field("floor", &self.floor)
            .finish()
    }
}

impl Default for Scheduler {
    fn default() -> Self {
        Scheduler::new()
    }
}

impl Scheduler {
    /// A scheduler with a 1 ms idle floor.
    #[must_use]
    pub fn new() -> Self {
        Scheduler {
            floor: Duration::from_millis(1),
        }
    }

    /// A scheduler that never sleeps less than `floor` while idle.
    #[must_use]
    pub fn with_idle_floor(floor: Duration) -> Self {
        Scheduler { floor }
    }

    /// One frame. Never blocks: this is [`crate::tick`] with the counts attached.
    pub fn step(&mut self) -> Tick {
        step()
    }

    /// Run frames until `should_stop` says so, installing a [`Driver`] for the
    /// duration. `should_stop` sees every frame, idle ones included.
    ///
    /// The driver thread is gone by the time this returns, so a test can start
    /// the next one immediately.
    pub fn run_until(&mut self, mut should_stop: impl FnMut(&Tick) -> bool) {
        let _driver = Driver::for_current_thread();
        loop {
            let tick = self.step();
            if should_stop(&tick) {
                return;
            }
            self.park_if_idle(&tick);
        }
    }

    /// Run frames until the program ends.
    pub fn run(&mut self) {
        let _driver = Driver::for_current_thread();
        loop {
            let tick = self.step();
            self.park_if_idle(&tick);
        }
    }

    /// Sleep when the frame had nothing to do.
    fn park_if_idle(&self, tick: &Tick) {
        if !tick.is_idle() {
            return;
        }
        match next_wakeup() {
            // Nothing at all is pending: block until a write, a wake or a new
            // timer arrives, all of which unpark this thread.
            None => thread::park(),
            Some(deadline) => {
                let wait = deadline
                    .saturating_duration_since(Instant::now())
                    .max(self.floor);
                thread::park_timeout(wait);
            }
        }
    }
}

/// What the driver thread is told.
enum Message {
    /// The earliest moment something needs attention. `None` means "nothing".
    Wake(Option<Instant>),
    Stop,
}

struct DriverState {
    tx: Mutex<Option<Sender<Message>>>,
}

thread_local! {
    /// The driver installed on this thread, if any.
    static DRIVER: RefCell<Option<Arc<DriverState>>> = const { RefCell::new(None) };
}

/// A background thread that knows when the next frame is due.
///
/// It holds no graph state and runs no user code: it sleeps until the earliest
/// armed deadline — or until the owner arms an earlier one — and then calls the
/// waker. A waker is typically `thread::current().unpark`, but any
/// `Fn() + Send + Sync` will do, which is how a GUI event loop or a socket
/// notifier gets its turn.
///
/// Dropping it stops the thread and waits for it, so a driver never outlives the
/// loop it was driving.
pub struct Driver {
    state: Arc<DriverState>,
    thread: Option<thread::JoinHandle<()>>,
}

impl std::fmt::Debug for Driver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Driver")
            .field("finished", &self.thread.as_ref().is_none_or(thread::JoinHandle::is_finished))
            .finish()
    }
}

impl Driver {
    /// Start a driver that calls `on_wake` when the next deadline passes, and
    /// install it as this thread's driver.
    ///
    /// The waker runs on the driver thread, so it must be `Send + Sync` — keep it
    /// to waking, never to touching the graph.
    #[must_use = "the driver stops when it is dropped"]
    pub fn new(on_wake: impl Fn() + Send + Sync + 'static) -> Self {
        let (tx, rx) = channel();
        let state = Arc::new(DriverState {
            tx: Mutex::new(Some(tx)),
        });
        DRIVER.with(|installed| {
            *installed.borrow_mut() = Some(state.clone());
        });

        let on_wake: Arc<dyn Fn() + Send + Sync> = Arc::new(on_wake);
        let driver_thread = thread::Builder::new()
            .name("rok-ui-hooks-driver".into())
            .spawn(move || drive(rx, on_wake))
            .expect("spawning the driver thread");

        // Tell the fresh thread what is already pending.
        driver_wake();
        Driver {
            state,
            thread: Some(driver_thread),
        }
    }

    /// A driver that unparks the calling thread — the pairing
    /// [`Scheduler::run_until`] and [`Scheduler::run`] rely on.
    #[must_use = "the driver stops when it is dropped"]
    pub fn for_current_thread() -> Self {
        Driver::new(|| thread::current().unpark())
    }
}

impl Drop for Driver {
    fn drop(&mut self) {
        DRIVER.with(|installed| {
            installed.borrow_mut().take();
        });
        let sender = self.state.tx.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(tx) = sender.as_ref() {
            let _ = tx.send(Message::Stop);
        }
        drop(sender);
        // The thread returns as soon as it sees the message. Joining here is what
        // guarantees no waker can fire after this point, and so that starting a
        // new driver cannot leave two of them watching the same graph.
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

/// Tell the installed driver that the set of deadlines changed.
pub(crate) fn deadline_changed() {
    driver_wake();
}

fn driver_wake() {
    let state = DRIVER.with(|installed| installed.borrow().clone());
    let Some(state) = state else {
        return; // no driver on this thread: nobody is listening
    };
    let guard = state.tx.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(tx) = guard.as_ref() {
        // A closed channel means the driver thread is already gone, which is not
        // worth reporting: the owner simply has no one to wake it.
        let _ = tx.send(Message::Wake(next_wakeup()));
    }
}

/// The driver thread's whole job.
fn drive(rx: Receiver<Message>, on_wake: Arc<dyn Fn() + Send + Sync>) {
    let mut next: Option<Instant> = None;
    loop {
        match next {
            None => match rx.recv() {
                Ok(Message::Wake(at)) => next = at,
                Ok(Message::Stop) | Err(_) => return,
            },
            Some(at) => {
                let now = Instant::now();
                if at <= now {
                    on_wake();
                    next = None;
                    continue;
                }
                match rx.recv_timeout(at - now) {
                    // An earlier deadline wins: a later one must not postpone a
                    // deadline that is already coming.
                    Ok(Message::Wake(earlier)) => {
                        next = Some(match earlier {
                            Some(earlier) => earlier.min(at),
                            None => at,
                        });
                    }
                    Ok(Message::Stop) | Err(RecvTimeoutError::Disconnected) => return,
                    Err(RecvTimeoutError::Timeout) => {
                        on_wake();
                        next = None;
                    }
                }
            }
        }
    }
}