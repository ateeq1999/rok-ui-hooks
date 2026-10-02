//! The reactive graph: nodes, owners, tracking, lazy evaluation and scheduling.
//!
//! Nothing in this module is public except [`untrack`], [`batch`], [`flush`],
//! [`tick`], [`create_root`], [`Root`] and [`on_cleanup`].
//!
//! # The algorithm
//!
//! Every signal, memo and effect is a [`Node`]. Nodes point at what they read
//! (`sources`, strong) and at what reads them (`subs`, weak, so a dropped
//! subscriber can never keep itself alive).
//!
//! A write runs nothing. It bumps the node's `version` and *marks* everything
//! downstream:
//!
//! | downstream node | on notification | action                   |
//! |-----------------|-----------------|--------------------------|
//! | memo            | `Clean`         | → `Check`, keep going    |
//! | memo            | `Check`         | keep going               |
//! | memo            | `Dirty`         | stop (already in motion) |
//! | effect          | `Clean`         | → `Check`, queue it      |
//!
//! Reading pulls. [`update_if_necessary`] makes sure every memo a node read is
//! up to date *before* comparing versions; if nothing actually moved, the node
//! settles back to `Clean` without running its body. That is what makes the
//! engine glitch-free for any graph shape, and what keeps memos lazy — nothing
//! is computed that nobody asks for.
//!
//! Effects are the only eager nodes. They are queued and drained oldest first
//! (queues are keyed by id, and ids follow creation order), so a run is
//! deterministic.

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::{Rc, Weak};

use crate::context::{ProviderFrame, capture_providers, with_captured_providers};
use crate::deps::DepsAny;

/// Identity of a node. Ids are handed out in creation order, which is also the
/// order effects and memos are flushed in.
pub(crate) type Id = u64;

/// How many times one node may run inside a single flush before we call the
/// graph cyclic. Real graphs never come close; a ping-pong between two effects
/// trips this immediately.
const MAX_EFFECT_RUNS: u32 = 100;

/// Depth guard for [`mark_downstream`]. It stops a memo cycle from blowing the
/// stack, and bounds how long a chain of memos may be.
const MAX_PROPAGATION_DEPTH: usize = 10_000;

type Queue = BTreeMap<Id, Weak<Node>>;

thread_local! {
    /// The observer stack. `None` entries mean "untracked" (see [`untrack`]).
    static OBSERVER: RefCell<Vec<Option<Rc<Node>>>> = const { RefCell::new(Vec::new()) };
    /// Ownership stack: computations created here are disposed with it.
    static OWNER_STACK: RefCell<Vec<Id>> = const { RefCell::new(Vec::new()) };
    /// Every live node, weakly held so owners can address nodes by id.
    static REGISTRY: RefCell<BTreeMap<Id, Weak<Node>>> = const { RefCell::new(BTreeMap::new()) };
    static NEXT_ID: Cell<Id> = const { Cell::new(0) };
    /// Effects waiting for the end of the batch.
    static PENDING: RefCell<Queue> = const { RefCell::new(BTreeMap::new()) };
    /// Effects waiting for an explicit [`flush`].
    static DEFERRED: RefCell<Queue> = const { RefCell::new(BTreeMap::new()) };
    /// Runs per node within one flush, to spot cycles.
    static RUNS: RefCell<BTreeMap<Id, u32>> = const { RefCell::new(BTreeMap::new()) };
    static BATCH_DEPTH: Cell<usize> = const { Cell::new(0) };
    static PROPAGATION_DEPTH: Cell<usize> = const { Cell::new(0) };
}

fn next_id() -> Id {
    NEXT_ID.with(|n| {
        let id = n.get();
        n.set(id + 1);
        id
    })
}

pub(crate) fn lookup(id: Id) -> Option<Rc<Node>> {
    REGISTRY.with(|r| r.borrow().get(&id).and_then(Weak::upgrade))
}

/// The computation currently running, if any.
pub(crate) fn current_observer() -> Option<Rc<Node>> {
    OBSERVER.with(|o| o.borrow().last().cloned().flatten())
}

/// The owner that would adopt a computation created right now.
pub(crate) fn current_owner() -> Option<Id> {
    OWNER_STACK.with(|o| o.borrow().last().copied())
}

/// Pops a thread-local stack even if the body panicked.
struct PopGuard;
impl Drop for PopGuard {
    fn drop(&mut self) {
        OBSERVER.with(|o| {
            o.borrow_mut().pop();
        });
    }
}

/// Runs `f` as `node`: observer context, ownership, and captured providers.
fn with_running<R>(node: &Rc<Node>, f: impl FnOnce() -> R) -> R {
    struct Restore {
        owned: bool,
    }
    impl Drop for Restore {
        fn drop(&mut self) {
            OBSERVER.with(|o| {
                o.borrow_mut().pop();
            });
            if self.owned {
                OWNER_STACK.with(|o| {
                    o.borrow_mut().pop();
                });
            }
        }
    }

    OBSERVER.with(|o| o.borrow_mut().push(Some(node.clone())));
    OWNER_STACK.with(|o| o.borrow_mut().push(node.id));
    let _restore = Restore { owned: true };
    with_captured_providers(&node.providers, f)
}

/// Keeps the batch depth balanced if `f` panics.
struct DepthGuard;
impl Drop for DepthGuard {
    fn drop(&mut self) {
        BATCH_DEPTH.with(|d| d.set(d.get().saturating_sub(1)));
    }
}

/// Bounds [`mark_downstream`] recursion, so a cyclic graph reports itself
/// instead of exhausting the stack.
struct PropagationGuard;
impl PropagationGuard {
    fn enter() -> Self {
        let depth = PROPAGATION_DEPTH.with(|d| {
            let depth = d.get();
            d.set(depth + 1);
            depth
        });
        assert!(
            depth < MAX_PROPAGATION_DEPTH,
            "reactive graph is cyclic, or deeper than {MAX_PROPAGATION_DEPTH} nodes: \
             mark_downstream did not converge"
        );
        PropagationGuard
    }
}
impl Drop for PropagationGuard {
    fn drop(&mut self) {
        PROPAGATION_DEPTH.with(|d| d.set(d.get().saturating_sub(1)));
    }
}

/// A node of the reactive graph: a signal, a memo, an effect, or an owner.
pub(crate) struct Node {
    pub(crate) id: Id,
    /// `true` for effects (reactions), `false` for signals, memos and owners.
    pub(crate) is_effect: bool,
    pub(crate) mode: Cell<Mode>,
    pub(crate) state: Cell<State>,
    /// Bumped only when the *value* changes, so downstream checks are exact.
    pub(crate) version: Cell<u64>,
    /// Memo body. Returns `true` when the new value differs from the old.
    compute: RefCell<Option<Box<dyn FnMut() -> bool>>>,
    /// Effect body.
    body: RefCell<Option<Box<dyn FnMut()>>>,
    /// What this node read, in read order, plus the versions it saw.
    sources: RefCell<Vec<Rc<Node>>>,
    source_versions: RefCell<Vec<u64>>,
    /// What reads this node, keyed by id so unsubscribing is exact.
    subs: RefCell<BTreeMap<Id, Weak<Node>>>,
    /// `on_cleanup` callbacks.
    pub(crate) cleanups: RefCell<Vec<Box<dyn FnOnce()>>>,
    /// Computations created while this node was running. Held **strongly**, so
    /// an owned computation outlives the local handle that created it and dies
    /// with its owner.
    children: RefCell<Vec<Rc<Node>>>,
    /// The declared deps gate, type-erased so nodes stay non-generic.
    deps: RefCell<Option<Rc<dyn DepsAny>>>,
    prev_deps: RefCell<Option<Box<dyn Any>>>,
    /// Providers visible where this node was created; re-installed on every
    /// run, so `use_context` resolves by creation site, not dynamic scope.
    providers: Vec<ProviderFrame>,
    owner: Cell<Option<Id>>,
    /// Set while the body runs: a node never runs inside itself.
    running: Cell<bool>,
    disposed: Cell<bool>,
}

/// Freshness of a node.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum State {
    /// Up to date.
    #[default]
    Clean,
    /// Something upstream may have changed; needs a version check to know.
    Check,
    /// Definitely stale.
    Dirty,
}

/// When an effect is allowed to run.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum Mode {
    /// At the end of the enclosing batch (the default).
    #[default]
    Eager,
    /// Only on an explicit [`flush`] — a manual "render frame".
    Deferred,
}

impl Node {
    /// Allocate a node and adopt it into the current owner.
    pub(crate) fn create(is_effect: bool) -> Rc<Self> {
        let id = next_id();
        let node = Rc::new(Node {
            id,
            is_effect,
            mode: Cell::new(Mode::Eager),
            state: Cell::new(State::Clean),
            version: Cell::new(0),
            compute: RefCell::new(None),
            body: RefCell::new(None),
            sources: RefCell::new(Vec::new()),
            source_versions: RefCell::new(Vec::new()),
            subs: RefCell::new(BTreeMap::new()),
            cleanups: RefCell::new(Vec::new()),
            children: RefCell::new(Vec::new()),
            deps: RefCell::new(None),
            prev_deps: RefCell::new(None),
            providers: capture_providers(),
            owner: Cell::new(current_owner()),
            running: Cell::new(false),
            disposed: Cell::new(false),
        });
        REGISTRY.with(|r| {
            r.borrow_mut().insert(id, Rc::downgrade(&node));
        });
        if let Some(owner) = current_owner() {
            adopt(owner, &node);
        }
        node
    }

    /// An inert node whose only job is to own computations.
    pub(crate) fn create_owner() -> Rc<Self> {
        Node::create(false)
    }

    pub(crate) fn is_disposed(&self) -> bool {
        self.disposed.get()
    }

    /// Attach the declared deps gate.
    pub(crate) fn set_deps(&self, deps: Rc<dyn DepsAny>) {
        *self.deps.borrow_mut() = Some(deps);
    }

    /// Attach the body of an effect.
    pub(crate) fn set_body(&self, body: Box<dyn FnMut()>) {
        *self.body.borrow_mut() = Some(body);
    }

    /// Attach the body of a memo. It returns `true` when the value changed.
    pub(crate) fn set_compute(&self, compute: Box<dyn FnMut() -> bool>) {
        *self.compute.borrow_mut() = Some(compute);
    }

    pub(crate) fn set_mode(&self, mode: Mode) {
        self.mode.set(mode);
    }

    /// Force the next read of this node to recompute it, gate open.
    pub(crate) fn invalidate(&self) {
        *self.prev_deps.borrow_mut() = None;
        self.state.set(State::Dirty);
    }

    /// Dispose everything this node owns and unlink its sources, without
    /// disposing the node itself. Used to re-run a render in the same scope.
    pub(crate) fn reset(&self) {
        self.tear_down_links();
    }

    /// Unlink from every source, run cleanups, dispose owned computations.
    fn tear_down_links(&self) {
        // The borrow must end before the loop: disposing a child calls back into
        // `release`, which borrows this same `children` list.
        let children = std::mem::take(&mut *self.children.borrow_mut());
        for child in children {
            child.tear_down();
        }
        let cleanups = std::mem::take(&mut *self.cleanups.borrow_mut());
        for cleanup in cleanups {
            cleanup();
        }
        let sources = std::mem::take(&mut *self.sources.borrow_mut());
        self.source_versions.borrow_mut().clear();
        for source in sources {
            source.subs.borrow_mut().remove(&self.id);
        }
    }

    /// Same as [`Node::tear_down_links`], plus marking the node inert so a stray
    /// handle to this node does nothing.
    fn tear_down(&self) {
        if self.disposed.get() {
            return;
        }
        self.disposed.set(true);
        self.tear_down_links();
        self.state.set(State::Clean);
        self.body.borrow_mut().take();
        self.compute.borrow_mut().take();
        if let Some(owner) = self.owner.get() {
            release(owner, self.id);
        }
        REGISTRY.with(|r| {
            r.borrow_mut().remove(&self.id);
        });
    }

    /// Run a fresh node once, on mount.
    pub(crate) fn mount(node: &Rc<Self>) {
        node.state.set(State::Dirty);
        update_if_necessary(node);
    }

    /// Subscribe the running computation to this node, both ways.
    pub(crate) fn track(node: &Rc<Self>) {
        let Some(running) = current_observer() else {
            return;
        };
        if running.id == node.id {
            return; // reading itself would be a cycle
        }
        if let Some(pos) = running
            .sources
            .borrow()
            .iter()
            .position(|s| s.id == node.id)
        {
            // Already linked during this run: refresh the recorded version.
            running.source_versions.borrow_mut()[pos] = node.version.get();
            return;
        }
        running.sources.borrow_mut().push(node.clone());
        running
            .source_versions
            .borrow_mut()
            .push(node.version.get());
        node.subs
            .borrow_mut()
            .insert(running.id, Rc::downgrade(&running));
    }

    /// The declared-deps gate, evaluated untracked so that comparing deps never
    /// subscribes the reader to the handles inside the tuple.
    fn open_gate(node: &Rc<Self>) -> bool {
        let Some(deps) = node.deps.borrow().clone() else {
            return true;
        };
        let next = untrack(|| deps.eval());
        let open = match &*node.prev_deps.borrow() {
            None => true,
            Some(prev) => deps.should_run(Some(prev.as_ref()), next.as_ref()),
        };
        if open {
            *node.prev_deps.borrow_mut() = Some(next);
        }
        open
    }

    /// Re-subscribe through the deps tuple, inside the tracked scope. Called
    /// after teardown, which dropped the previous links.
    fn relink(&self) {
        if let Some(deps) = self.deps.borrow().clone() {
            deps.eval();
        }
    }
}

impl Drop for Node {
    fn drop(&mut self) {
        self.tear_down();
    }
}

/// Publish a new value: bump the version, then mark everything downstream.
pub(crate) fn notify(node: &Rc<Node>) {
    node.version.set(node.version.get() + 1);
    run_updates(|| mark_downstream(node));
}

/// Mark downstream nodes stale: memos become `Check` and pass the mark on,
/// effects become `Check` and get queued.
fn mark_downstream(node: &Rc<Node>) {
    let _guard = PropagationGuard::enter();
    let subs: Vec<Rc<Node>> = node
        .subs
        .borrow()
        .values()
        .filter_map(Weak::upgrade)
        .collect();
    for sub in subs {
        if sub.is_disposed() || sub.running.get() {
            continue;
        }
        if sub.is_effect {
            if sub.state.get() == State::Clean {
                sub.state.set(State::Check);
                queue(&sub);
            }
        } else {
            match sub.state.get() {
                State::Clean => {
                    sub.state.set(State::Check);
                    mark_downstream(&sub);
                }
                State::Check => mark_downstream(&sub),
                State::Dirty => {}
            }
        }
    }
}

/// Bring `node` up to date, pulling the memos it read first.
pub(crate) fn update_if_necessary(node: &Rc<Node>) {
    if node.is_disposed() || node.running.get() || node.state.get() == State::Clean {
        return;
    }

    let sources = node.sources.borrow().clone();
    for source in &sources {
        if !source.is_effect && !source.is_disposed() {
            update_if_necessary(source);
        }
    }

    if node.state.get() == State::Check && !sources_changed(node, &sources) {
        node.state.set(State::Clean); // nothing moved: settle without running
        record_versions(node, &sources);
        return;
    }

    node.state.set(State::Dirty);
    if node.is_effect {
        run_effect(node);
    } else {
        recompute(node);
    }
}

fn sources_changed(node: &Rc<Node>, sources: &[Rc<Node>]) -> bool {
    let versions = node.source_versions.borrow();
    sources
        .iter()
        .zip(versions.iter())
        .any(|(source, seen)| source.version.get() != *seen)
}

fn record_versions(node: &Rc<Node>, sources: &[Rc<Node>]) {
    let mut versions = node.source_versions.borrow_mut();
    versions.clear();
    versions.extend(sources.iter().map(|s| s.version.get()));
}

/// Run an effect: deps gate → teardown → relink → body.
fn run_effect(node: &Rc<Node>) {
    if node.running.get() {
        return;
    }
    node.running.set(true);
    if Node::open_gate(node) {
        node.tear_down_links();
        with_running(node, || {
            node.relink();
            let mut slot = node.body.borrow_mut();
            if let Some(body) = slot.as_mut() {
                body();
            }
        });
    }
    node.running.set(false);
    node.state.set(State::Clean);
    let sources = node.sources.borrow().clone();
    record_versions(node, &sources);
}

/// Recompute a memo. Subscribers are marked only when the value really changed.
fn recompute(node: &Rc<Node>) {
    if node.running.get() {
        return;
    }
    node.running.set(true);
    let changed = if Node::open_gate(node) {
        node.tear_down_links();
        with_running(node, || {
            node.relink();
            let mut slot = node.compute.borrow_mut();
            match slot.as_mut() {
                Some(compute) => compute(),
                None => false,
            }
        })
    } else {
        false
    };
    node.running.set(false);
    node.state.set(State::Clean);
    let sources = node.sources.borrow().clone();
    record_versions(node, &sources);

    if changed && !node.is_disposed() {
        node.version.set(node.version.get() + 1);
        mark_downstream(node);
    }
}

/// Dispose a node and everything it owns, by id.
pub(crate) fn dispose_node(id: Id) {
    if let Some(node) = lookup(id) {
        node.tear_down();
    }
}

fn adopt(owner: Id, child: &Rc<Node>) {
    if let Some(node) = lookup(owner) {
        node.children.borrow_mut().push(child.clone());
    }
}

fn release(owner: Id, child: Id) {
    if let Some(node) = lookup(owner) {
        node.children.borrow_mut().retain(|c| c.id != child);
    }
}

fn queue(node: &Rc<Node>) {
    match node.mode.get() {
        Mode::Eager => PENDING.with(|p| {
            p.borrow_mut().insert(node.id, Rc::downgrade(node));
        }),
        Mode::Deferred => DEFERRED.with(|p| {
            p.borrow_mut().insert(node.id, Rc::downgrade(node));
        }),
    }
}

fn queue_len(deferred: bool) -> usize {
    if deferred {
        DEFERRED.with(|p| p.borrow().len())
    } else {
        PENDING.with(|p| p.borrow().len())
    }
}

/// Pop the oldest queued node of one queue, or `None` when it is empty.
fn pop_next(deferred: bool) -> Option<(Id, Weak<Node>)> {
    if deferred {
        DEFERRED.with(|p| p.borrow_mut().pop_first())
    } else {
        PENDING.with(|p| p.borrow_mut().pop_first())
    }
}

fn count_run(id: Id) {
    let overflow = RUNS.with(|r| {
        let mut runs = r.borrow_mut();
        let count = runs.entry(id).or_insert(0);
        *count += 1;
        *count > MAX_EFFECT_RUNS
    });
    assert!(
        !overflow,
        "effect loop detected: an effect keeps writing to what it reads \
         (one node ran {MAX_EFFECT_RUNS} times in a single flush)"
    );
}

/// Run `f`, then drain the eager queue once the outermost batch ends.
pub(crate) fn run_updates<R>(f: impl FnOnce() -> R) -> R {
    BATCH_DEPTH.with(|d| d.set(d.get() + 1));
    let _depth = DepthGuard;
    let result = f();

    // Only the outermost batch flushes, and it stays at depth 1 while doing so:
    // writes made *by* effects are queued, never run re-entrantly. Oldest first,
    // so a memo is refreshed before the effects that read it.
    if BATCH_DEPTH.with(|d| d.get()) == 1 {
        drain(false);
    }
    result
}

fn drain(deferred: bool) {
    RUNS.with(|r| r.borrow_mut().clear());
    while let Some((id, weak)) = pop_next(deferred) {
        if let Some(node) = weak.upgrade() {
            count_run(id);
            update_if_necessary(&node);
        }
    }
    RUNS.with(|r| r.borrow_mut().clear());
}

/// Run `f` without subscribing the current observer to anything it reads.
///
/// ```
/// use rok_ui_hooks::*;
///
/// let (a, set_a) = use_state(1);
/// let (b, set_b) = use_state(10);
/// let seen = use_memo(
///     {
///         let (a, b) = (a.clone(), b.clone());
///         move || a.get() + untrack(|| b.get())
///     },
///     (a,),
/// );
///
/// set_b.set(20);      // `b` is read untracked: nobody is notified
/// set_a.set(2);
/// assert_eq!(seen.get(), 22);
/// ```
pub fn untrack<R>(f: impl FnOnce() -> R) -> R {
    let _guard = PopGuard;
    OBSERVER.with(|o| o.borrow_mut().push(None));
    f()
}

/// Group writes: effects are queued and each one runs at most once per flush.
///
/// ```
/// use rok_ui_hooks::*;
///
/// let (a, set_a) = use_state(1);
/// let (b, set_b) = use_state(2);
/// let total = use_memo(
///     { let (a, b) = (a.clone(), b.clone()); move || a.get() + b.get() },
///     (),
/// );
/// let printed = use_memo(
///     { let total = total.clone(); move || total.get() * 10 },
///     (),
/// );
///
/// batch(|| {
///     set_a.set(10);
///     set_b.set(20);
/// });
/// // Still fresh *inside* the batch: memos pull when they are read.
/// assert_eq!(printed.get(), 300);
/// ```
pub fn batch<R>(f: impl FnOnce() -> R) -> R {
    run_updates(f)
}

/// Run every queued effect, deferred ones included.
///
/// ```
/// use rok_ui_hooks::*;
///
/// let (a, set_a) = use_state(0);
/// let _e = create_deferred_effect(
///     { let a = a.clone(); move || println!("a = {}", a.get()) },
///     (),
/// );
/// set_a.set(1); // queued, not run
/// flush();      // now it runs
/// ```
pub fn flush() {
    run_updates(|| {
        while queue_len(false) > 0 || queue_len(true) > 0 {
            drain(false);
            drain(true);
        }
    });
}

/// Advance the world by one frame: fire due timers, poll woken tasks, then run
/// every pending effect.
///
/// This is the single call a UI loop needs per frame. Outside a UI the runtime
/// already reacts to writes, so call it only when you use timers, async
/// resources or deferred effects.
pub fn tick() {
    crate::timer::drain();
    crate::executor::run_ready();
    flush();
}

/// A disposable scope. Computations created inside [`Root::run`] — directly or
/// in an effect body — are disposed together when the root goes away.
///
/// ```
/// use rok_ui_hooks::*;
///
/// let (count, set_count) = use_state(0);
/// let ran = std::rc::Rc::new(std::cell::Cell::new(0));
///
/// {
///     let root = Root::new();
///     let r = ran.clone();
///     root.run(move || {
///         let c = count.clone();
///         let _e = use_effect(move || {
///             c.get();
///             r.set(r.get() + 1);
///         }, ());
///     });
///     set_count.set(1);
///     assert_eq!(ran.get(), 2);
/// }
///
/// // The root is gone: the effect is disposed, writes are inert.
/// set_count.set(2);
/// assert_eq!(ran.get(), 2);
/// ```
pub struct Root {
    id: Id,
    /// Held so the owner node outlives this handle; the registry is weak.
    _node: Rc<Node>,
}

impl std::fmt::Debug for Root {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Root").field("id", &self.id).finish()
    }
}

impl Default for Root {
    fn default() -> Self {
        Root::new()
    }
}

impl Root {
    /// An empty scope. Dispose it by dropping it.
    #[must_use]
    pub fn new() -> Self {
        let node = Node::create_owner();
        Root {
            id: node.id,
            _node: node,
        }
    }

    /// Run `f` inside this root. Everything created here belongs to it.
    pub fn run<R>(&self, f: impl FnOnce() -> R) -> R {
        run_in_owner(self.id, f)
    }

    /// Dispose the scope now. Same as dropping it, but explicit — and reusable:
    /// a disposed root owns nothing, so it can still be [`Root::run`] again.
    pub fn dispose(&self) {
        self._node.tear_down();
    }
}

impl Drop for Root {
    fn drop(&mut self) {
        dispose_node(self.id);
    }
}

/// Run `f` in a fresh root and dispose it before returning — the scoped form of
/// [`Root`].
///
/// ```
/// use rok_ui_hooks::*;
///
/// let (n, set_n) = use_state(0);
/// let ran = std::rc::Rc::new(std::cell::Cell::new(0));
///
/// create_root(|| {
///     let r = ran.clone();
///     let s = n.clone();
///     let _e = use_effect(move || {
///         s.get();
///         r.set(r.get() + 1);
///     }, ());
/// });
/// set_n.set(1);
/// assert_eq!(ran.get(), 1); // disposed with the root
/// ```
pub fn create_root<R>(f: impl FnOnce() -> R) -> R {
    let root = Root::new();
    root.run(f)
}

/// Run `f` with `owner` on the ownership stack. Public so the crate's own
/// containers can own computations.
pub(crate) fn run_in_owner<R>(owner: Id, f: impl FnOnce() -> R) -> R {
    struct PopOwner(Option<Id>);
    impl Drop for PopOwner {
        fn drop(&mut self) {
            if self.0.is_some() {
                OWNER_STACK.with(|o| {
                    o.borrow_mut().pop();
                });
            }
        }
    }

    OWNER_STACK.with(|o| o.borrow_mut().push(owner));
    let _pop = PopOwner(Some(owner));
    f()
}

/// `onCleanup(fn)` — register teardown for whatever is running right now: the
/// computation, else the nearest owner.
pub fn on_cleanup(f: impl FnOnce() + 'static) {
    if let Some(node) = current_observer() {
        node.cleanups.borrow_mut().push(Box::new(f));
        return;
    }
    if let Some(owner) = current_owner() {
        if let Some(node) = lookup(owner) {
            node.cleanups.borrow_mut().push(Box::new(f));
        }
    }
}
