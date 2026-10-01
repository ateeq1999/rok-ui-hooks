//! A tiny fine-grained reactive library (signals, effects, memos) in Rust,
//! modelled on Ryan Carniato's "Building a Reactive Library from Scratch".

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::{Rc, Weak};

// ───────────────────────────── Runtime (global state) ─────────────────────────────

type ComputationId = u64;

thread_local! {
    /// The `context` stack from the article. `None` entries mean "untracked".
    static OBSERVER: RefCell<Vec<Option<Rc<Computation>>>> = const { RefCell::new(Vec::new()) };
    static NEXT_ID: Cell<ComputationId> = const { Cell::new(0) };
    static BATCH_DEPTH: Cell<usize> = const { Cell::new(0) };
    /// Queue of computations to run, ordered by id (= creation order).
    static PENDING: RefCell<BTreeMap<ComputationId, Weak<Computation>>> = const { RefCell::new(BTreeMap::new()) };
}

fn next_id() -> ComputationId {
    NEXT_ID.with(|n| {
        let id = n.get();
        n.set(id + 1);
        id
    })
}

fn current_observer() -> Option<Rc<Computation>> {
    OBSERVER.with(|o| o.borrow().last().cloned().flatten())
}

/// Push `obs` on the context stack, run `f`, and pop again — even if `f` panics.
fn with_observer<R>(obs: Option<Rc<Computation>>, f: impl FnOnce() -> R) -> R {
    struct PopGuard;
    impl Drop for PopGuard {
        fn drop(&mut self) {
            OBSERVER.with(|o| {
                o.borrow_mut().pop();
            });
        }
    }
    OBSERVER.with(|o| o.borrow_mut().push(obs));
    let _guard = PopGuard;
    f()
}

// ───────────────────────────── Source (anything trackable) ─────────────────────────────

/// Type-erased view of a signal so a computation can unsubscribe from signals of any `T`.
trait Source {
    fn unsubscribe(&self, id: ComputationId);
}

// ───────────────────────────── Computation (the "running" object) ─────────────────────────────

struct Computation {
    id: ComputationId,
    f: RefCell<Box<dyn FnMut()>>,
    /// Back-links: every signal this computation read during its last run.
    sources: RefCell<Vec<Rc<dyn Source>>>,
    /// User callbacks registered with `on_cleanup` during the last run.
    cleanups: RefCell<Vec<Box<dyn FnOnce()>>>,
}

impl Computation {
    fn new(f: impl FnMut() + 'static) -> Rc<Self> {
        Rc::new(Computation {
            id: next_id(),
            f: RefCell::new(Box::new(f)),
            sources: RefCell::new(Vec::new()),
            cleanups: RefCell::new(Vec::new()),
        })
    }

    /// `execute()` from the article: cleanup → push context → run fn → pop context.
    fn run(self: &Rc<Self>) {
        // If `f` is already borrowed, this computation is currently running
        // (it wrote to a signal it reads). Skip to avoid infinite recursion.
        let Ok(mut f) = self.f.try_borrow_mut() else {
            return;
        };
        self.cleanup();
        with_observer(Some(self.clone()), &mut *f);
    }

    /// Unsubscribe from every source and run user cleanup callbacks.
    fn cleanup(&self) {
        let sources = std::mem::take(&mut *self.sources.borrow_mut());
        for source in sources {
            source.unsubscribe(self.id);
        }
        let cleanups = std::mem::take(&mut *self.cleanups.borrow_mut());
        for c in cleanups {
            c();
        }
    }

    fn is_running(&self) -> bool {
        self.f.try_borrow_mut().is_err()
    }

    /// Run now, or queue if we are inside a batch.
    fn schedule(self: &Rc<Self>) {
        if self.is_running() {
            return;
        }
        if BATCH_DEPTH.with(|d| d.get()) > 0 {
            // Keyed by id → duplicates collapse, and the queue stays sorted.
            PENDING.with(|p| {
                p.borrow_mut().insert(self.id, Rc::downgrade(self));
            });
        } else {
            self.run();
        }
    }
}

impl Drop for Computation {
    fn drop(&mut self) {
        self.cleanup();
    }
}

// ───────────────────────────── Signal ─────────────────────────────

struct SignalInner<T> {
    value: RefCell<T>,
    /// Ordered by id so effects re-run in creation order (deterministic).
    subscribers: RefCell<BTreeMap<ComputationId, Weak<Computation>>>,
}

impl<T> Source for SignalInner<T> {
    fn unsubscribe(&self, id: ComputationId) {
        self.subscribers.borrow_mut().remove(&id);
    }
}

impl<T: 'static> SignalInner<T> {
    /// `subscribe(running, subscriptions)` from the article — a two-way link.
    fn track(self: &Rc<Self>) {
        if let Some(running) = current_observer() {
            let newly_added = self
                .subscribers
                .borrow_mut()
                .insert(running.id, Rc::downgrade(&running))
                .is_none();
            if newly_added {
                running.sources.borrow_mut().push(self.clone() as Rc<dyn Source>);
            }
        }
    }

    /// Notify subscribers. Snapshot first (`[...subscriptions]` in JS),
    /// because running a subscriber mutates the subscriber list.
    fn notify(&self) {
        let subs: Vec<Rc<Computation>> = {
            let mut map = self.subscribers.borrow_mut();
            map.retain(|_, w| w.strong_count() > 0);
            map.values().filter_map(Weak::upgrade).collect()
        };
        // Wrapping in `batch` makes every write a transaction: downstream
        // effects are queued and each runs once, after upstream memos settle.
        batch(|| {
            for sub in subs {
                sub.schedule();
            }
        });
    }
}

/// The getter half of a signal.
pub struct ReadSignal<T> {
    inner: Rc<SignalInner<T>>,
}

/// The setter half of a signal.
pub struct WriteSignal<T> {
    inner: Rc<SignalInner<T>>,
}

impl<T> Clone for ReadSignal<T> {
    fn clone(&self) -> Self {
        Self { inner: self.inner.clone() }
    }
}
impl<T> Clone for WriteSignal<T> {
    fn clone(&self) -> Self {
        Self { inner: self.inner.clone() }
    }
}

/// `createSignal(value)` → `(getter, setter)`.
pub fn create_signal<T: 'static>(value: T) -> (ReadSignal<T>, WriteSignal<T>) {
    let inner = Rc::new(SignalInner {
        value: RefCell::new(value),
        subscribers: RefCell::new(BTreeMap::new()),
    });
    (ReadSignal { inner: inner.clone() }, WriteSignal { inner })
}

impl<T: 'static> ReadSignal<T> {
    /// Tracked read by reference (no clone needed).
    pub fn with<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        self.inner.track();
        f(&self.inner.value.borrow())
    }

    /// Tracked read that clones the value.
    pub fn get(&self) -> T
    where
        T: Clone,
    {
        self.with(T::clone)
    }

    /// Read without subscribing the current observer.
    pub fn with_untracked<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        f(&self.inner.value.borrow())
    }

    pub fn get_untracked(&self) -> T
    where
        T: Clone,
    {
        self.with_untracked(T::clone)
    }
}

impl<T: 'static> WriteSignal<T> {
    pub fn set(&self, value: T) {
        *self.inner.value.borrow_mut() = value;
        self.inner.notify();
    }

    /// Mutate in place (great for Vec/HashMap signals).
    pub fn update(&self, f: impl FnOnce(&mut T)) {
        f(&mut self.inner.value.borrow_mut());
        self.inner.notify();
    }
}

// ───────────────────────────── Effect ─────────────────────────────

/// Owning handle to a running effect. Dropping it disposes the effect.
#[must_use = "dropping an Effect immediately disposes it"]
pub struct Effect {
    _computation: Rc<Computation>,
}

impl Effect {
    /// Stop the effect now (same as dropping it).
    pub fn dispose(self) {}

    /// Keep the effect alive for the rest of the program.
    pub fn leak(self) {
        std::mem::forget(self);
    }
}

/// `createEffect(fn)`: run `f` now and again whenever anything it read changes.
pub fn create_effect(f: impl FnMut() + 'static) -> Effect {
    let computation = Computation::new(f);
    computation.run();
    Effect { _computation: computation }
}

/// Register a callback that runs before the current effect re-runs or is disposed.
pub fn on_cleanup(f: impl FnOnce() + 'static) {
    if let Some(running) = current_observer() {
        running.cleanups.borrow_mut().push(Box::new(f));
    }
}

// ───────────────────────────── Memo (derivation) ─────────────────────────────

/// A cached derived value. It is itself trackable, like a signal.
pub struct Memo<T> {
    read: ReadSignal<T>,
    _effect: Rc<Effect>,
}

impl<T> Clone for Memo<T> {
    fn clone(&self) -> Self {
        Self { read: self.read.clone(), _effect: self._effect.clone() }
    }
}

/// `createMemo(fn)`: an effect that writes into a private signal,
/// but only when the new value differs from the old one.
pub fn create_memo<T: PartialEq + 'static>(mut f: impl FnMut() -> T + 'static) -> Memo<T> {
    type Slot<T> = Rc<RefCell<Option<(ReadSignal<T>, WriteSignal<T>)>>>;
    let slot: Slot<T> = Rc::new(RefCell::new(None));

    let slot_in_effect = slot.clone();
    let effect = create_effect(move || {
        let next = f(); // tracked: the memo subscribes to what `f` reads
        let existing = slot_in_effect.borrow().clone();
        match existing {
            None => *slot_in_effect.borrow_mut() = Some(create_signal(next)),
            Some((read, write)) => {
                if read.with_untracked(|old| *old != next) {
                    write.set(next);
                }
            }
        }
    });

    let (read, _write) = slot.borrow().clone().expect("memo ran once on creation");
    Memo { read, _effect: Rc::new(effect) }
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

// ───────────────────────────── untrack & batch ─────────────────────────────

/// Run `f` without subscribing the current observer to anything it reads.
pub fn untrack<R>(f: impl FnOnce() -> R) -> R {
    with_observer(None, f)
}

/// Group writes: effects are queued and each one runs at most once per flush.
pub fn batch<R>(f: impl FnOnce() -> R) -> R {
    BATCH_DEPTH.with(|d| d.set(d.get() + 1));
    let result = f();

    // Only the outermost batch flushes. We stay at depth 1 while flushing so
    // writes made *by* effects are queued too, instead of running re-entrantly.
    // Always run the OLDEST pending computation next: a memo is created before
    // anything that reads it, so it gets refreshed before its readers run.
    if BATCH_DEPTH.with(|d| d.get()) == 1 {
        while let Some((_, weak)) = PENDING.with(|p| p.borrow_mut().pop_first()) {
            if let Some(c) = weak.upgrade() {
                c.run();
            }
        }
    }

    BATCH_DEPTH.with(|d| d.set(d.get() - 1));
    result
}

// ───────────────────────────── Tests ─────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn log() -> (Rc<RefCell<Vec<String>>>, impl Fn(String) + Clone) {
        let l = Rc::new(RefCell::new(Vec::new()));
        let l2 = l.clone();
        (l, move |s: String| l2.borrow_mut().push(s))
    }

    #[test]
    fn signal_get_set() {
        let (count, set_count) = create_signal(3);
        assert_eq!(count.get(), 3);
        set_count.set(5);
        assert_eq!(count.get(), 5);
        set_count.update(|c| *c *= 2);
        assert_eq!(count.get(), 10);
    }

    #[test]
    fn effect_reruns_on_change() {
        let (logs, push) = log();
        let (count, set_count) = create_signal(0);
        let _e = create_effect(move || push(format!("The count is {}", count.get())));
        set_count.set(5);
        set_count.set(10);
        assert_eq!(*logs.borrow(), ["The count is 0", "The count is 5", "The count is 10"]);
    }

    #[test]
    fn memo_computes_once_for_many_readers() {
        let (logs, push) = log();
        let (first, set_first) = create_signal("John".to_string());
        let (last, _set_last) = create_signal("Smith".to_string());
        let p = push.clone();
        let full = create_memo(move || {
            p("compute".into());
            format!("{} {}", first.get(), last.get())
        });
        let (f1, p1) = (full.clone(), push.clone());
        let _a = create_effect(move || p1(format!("My name is {}", f1.get())));
        let (f2, p2) = (full.clone(), push.clone());
        let _b = create_effect(move || p2(format!("Your name is not {}", f2.get())));
        set_first.set("Jacob".into());
        assert_eq!(
            *logs.borrow(),
            [
                "compute",
                "My name is John Smith",
                "Your name is not John Smith",
                "compute",
                "My name is Jacob Smith",
                "Your name is not Jacob Smith",
            ]
        );
    }

    #[test]
    fn dynamic_dependencies() {
        let (logs, push) = log();
        let (first, _) = create_signal("John".to_string());
        let (last, set_last) = create_signal("Smith".to_string());
        let (show_full, set_show_full) = create_signal(true);
        let display = create_memo(move || {
            if !show_full.get() {
                return first.get();
            }
            format!("{} {}", first.get(), last.get())
        });
        let _e = create_effect(move || push(format!("My name is {}", display.get())));

        set_show_full.set(false);
        set_last.set("Legend".into()); // nobody listens to `last` now → no log
        set_show_full.set(true);
        assert_eq!(
            *logs.borrow(),
            ["My name is John Smith", "My name is John", "My name is John Legend"]
        );
    }

    #[test]
    fn on_cleanup_runs_before_rerun_and_on_dispose() {
        let (logs, push) = log();
        let (n, set_n) = create_signal(0);
        let p = push.clone();
        let e = create_effect(move || {
            let v = n.get();
            p(format!("run {v}"));
            let p2 = p.clone();
            on_cleanup(move || p2(format!("cleanup {v}")));
        });
        set_n.set(1);
        drop(e);
        set_n.set(2); // disposed → nothing
        assert_eq!(*logs.borrow(), ["run 0", "cleanup 0", "run 1", "cleanup 1"]);
    }

    #[test]
    fn untrack_does_not_subscribe() {
        let (logs, push) = log();
        let (a, set_a) = create_signal(1);
        let (b, set_b) = create_signal(10);
        let _e = create_effect(move || {
            let b_val = untrack(|| b.get());
            push(format!("{}", a.get() + b_val));
        });
        set_b.set(20); // untracked → no rerun
        set_a.set(2);
        assert_eq!(*logs.borrow(), ["11", "22"]);
    }

    #[test]
    fn batch_runs_effect_once() {
        let (logs, push) = log();
        let (a, set_a) = create_signal(1);
        let (b, set_b) = create_signal(2);
        let c = create_memo(move || b.get() * 2);
        let _e = create_effect(move || push(format!("sum {}", a.get() + c.get())));
        batch(|| {
            set_a.set(2);
            set_b.set(3);
        });
        assert_eq!(*logs.borrow(), ["sum 5", "sum 8"]);
    }

    #[test]
    fn diamond_is_glitch_free() {
        let (logs, push) = log();
        let (a, set_a) = create_signal(1);
        let a2 = a.clone();
        let double = create_memo(move || a2.get() * 2);
        let triple = create_memo(move || a.get() * 3);
        let _e = create_effect(move || push(format!("{} {}", double.get(), triple.get())));
        set_a.set(2);
        // Without the batch in `notify` we'd also see the inconsistent "4 3".
        assert_eq!(*logs.borrow(), ["2 3", "4 6"]);
    }

    #[test]
    fn memo_skips_equal_values() {
        let (logs, push) = log();
        let (n, set_n) = create_signal(1);
        let is_even = create_memo(move || n.get() % 2 == 0);
        let _e = create_effect(move || push(format!("even={}", is_even.get())));
        set_n.set(3); // still odd → memo value unchanged → effect not rerun
        set_n.set(4);
        assert_eq!(*logs.borrow(), ["even=false", "even=true"]);
    }

    #[test]
    fn self_write_does_not_loop() {
        let (n, set_n) = create_signal(0);
        let runs = Rc::new(Cell::new(0));
        let r = runs.clone();
        let n2 = n.clone();
        let _e = create_effect(move || {
            r.set(r.get() + 1);
            let v = n2.get();
            if v < 100 {
                set_n.set(v + 1); // would recurse forever without the guard
            }
        });
        assert_eq!(runs.get(), 1);
        assert_eq!(n.get_untracked(), 1);
    }

    #[test]
    fn nested_effects_track_correctly() {
        let (logs, push) = log();
        let (name, set_name) = create_signal("a");
        let (value, _) = create_signal(1);
        let p = push.clone();
        let _outer = create_effect(move || {
            let (p2, value) = (p.clone(), value.clone());
            // The inner effect is owned by the outer run: dispose it when the outer re-runs.
            let inner = create_effect(move || p2(format!("value {}", value.get())));
            on_cleanup(move || drop(inner));
            p(format!("name {}", name.get())); // read AFTER inner effect: must still track outer
        });
        set_name.set("b");
        assert_eq!(*logs.borrow(), ["value 1", "name a", "value 1", "name b"]);
    }
}
