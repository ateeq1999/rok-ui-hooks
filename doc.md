# Fine-Grained Reactivity in Rust — Signals, Effects & Memos from Scratch

A step-by-step guide to implementing the pattern from Ryan Carniato's two articles
([A Hands-on Introduction to Fine-Grained Reactivity](https://dev.to/ryansolid/a-hands-on-introduction-to-fine-grained-reactivity-3ndf)
and [Building a Reactive Library from Scratch](https://dev.to/ryansolid/building-a-reactive-library-from-scratch-1i0p))
in safe Rust, with zero dependencies.

By the end you will have a ~350-line crate with:

| Solid (JS)            | This crate (Rust)                     | Role                                   |
|-----------------------|---------------------------------------|----------------------------------------|
| `createSignal(v)`     | `create_signal(v) -> (ReadSignal, WriteSignal)` | Source of truth                |
| `createEffect(fn)`    | `create_effect(fn) -> Effect`         | Reaction / side effect                 |
| `createMemo(fn)`      | `create_memo(fn) -> Memo<T>`          | Cached derivation                      |
| `onCleanup(fn)`       | `on_cleanup(fn)`                      | Teardown before re-run / dispose       |
| `untrack(fn)`         | `untrack(fn)`                         | Read without subscribing               |
| `batch(fn)`           | `batch(fn)`                           | Transaction, each effect runs once     |

Everything in this document was compiled and tested (Rust 1.95, 11 passing tests, clippy clean).

---

## Table of contents

1. [The mental model](#1-the-mental-model)
2. [Translating JS to Rust: the design decisions](#2-translating-js-to-rust-the-design-decisions)
3. [Step 1 — Project setup](#step-1--project-setup)
4. [Step 2 — A plain (non-reactive) signal](#step-2--a-plain-non-reactive-signal)
5. [Step 3 — The observer context stack](#step-3--the-observer-context-stack)
6. [Step 4 — The `Computation` (the "running" object)](#step-4--the-computation-the-running-object)
7. [Step 5 — Make signals reactive: `track` and `notify`](#step-5--make-signals-reactive-track-and-notify)
8. [Step 6 — Effects with ownership](#step-6--effects-with-ownership)
9. [Step 7 — Dynamic dependencies, cleanup and `on_cleanup`](#step-7--dynamic-dependencies-cleanup-and-on_cleanup)
10. [Step 8 — Memos (derivations)](#step-8--memos-derivations)
11. [Step 9 — `untrack`](#step-9--untrack)
12. [Step 10 — `batch` and glitch-free updates](#step-10--batch-and-glitch-free-updates)
13. [Step 11 — Safety nets: re-entrancy, panics, `Drop`](#step-11--safety-nets-re-entrancy-panics-drop)
14. [Step 12 — Tests](#step-12--tests)
15. [Step 13 — Run the article's demo](#step-13--run-the-articles-demo)
16. [Pitfalls & limitations](#pitfalls--limitations)
17. [Where to go next](#where-to-go-next)
18. [Appendix — Full source of `src/lib.rs`](#appendix--full-source-of-srclibrs)

---

## 1. The mental model

Fine-grained reactivity is a **graph** with three kinds of nodes:

- **Signals** — hold a value. They have no dependencies. When written, they notify subscribers.
- **Reactions (effects)** — run a function. They *depend on* signals and produce side effects.
- **Derivations (memos)** — both at once: they depend on signals *and* can be depended on.

```
   ┌──────────┐   ┌──────────┐   ┌──────────────┐
   │ first    │   │ last     │   │ show_full    │     ← signals
   └────┬─────┘   └────┬─────┘   └──────┬───────┘
        │              │ (only when     │
        │              │  show_full)    │
        └──────────────┼────────────────┘
                       ▼
               ┌───────────────┐
               │ display_name  │                          ← memo
               └───────┬───────┘
                       ▼
               ┌───────────────┐
               │ println!(...) │                          ← effect
               └───────────────┘
```

The whole trick is **automatic dependency tracking**:

1. There is a global **context stack** of "currently running computations".
2. When a computation runs, it pushes itself onto the stack.
3. When a signal is *read*, it looks at the top of the stack and links itself to that computation — **both ways**:
   - `signal.subscribers` gets the computation (so the signal knows whom to notify),
   - `computation.sources` gets the signal (so the computation can unsubscribe later).
4. When a signal is *written*, it re-runs every subscriber.
5. Before every re-run, a computation **unsubscribes from everything** and rebuilds its dependency list from scratch. That is what makes dependencies *dynamic* (conditional reads just work).

> Ryan, answering a reader: the link is two-way because "the computation needs to also remove itself from the signal".

That's the entire algorithm. The rest of this guide is about expressing it well in Rust.

---

## 2. Translating JS to Rust: the design decisions

JavaScript lets you share mutable objects freely and a GC cleans up cycles. Rust makes us decide four things explicitly.

### 2.1 Shared ownership + interior mutability → `Rc<RefCell<…>>`

A signal is shared by its getter, its setter, and every computation that read it. All of them must mutate shared state (the value, the subscriber list). So: `Rc` for sharing, `RefCell` for mutation. This makes the library **single-threaded** (`!Send`), which is exactly what UI reactivity (Solid, Leptos on the client, Sycamore) uses.

### 2.2 The global context → `thread_local!`

JS has a module-level `const context = []`. Rust's equivalent for a single-threaded runtime is `thread_local!`. Each thread gets its own independent reactive world.

### 2.3 Who owns whom? (avoiding `Rc` cycles)

The graph has links in both directions. If both were `Rc`, every signal↔effect pair would be a reference cycle and leak. Pick one direction to be strong:

| Link                                    | Kind        | Why                                                            |
|-----------------------------------------|-------------|----------------------------------------------------------------|
| `Computation.sources → Signal`          | `Rc` (strong)  | An effect needs its signals to stay alive while it depends on them |
| `Signal.subscribers → Computation`      | `Weak`      | A signal must **not** keep effects alive                        |
| `Effect` handle → `Computation`         | `Rc` (strong)  | **You** own the effect; dropping the handle disposes it         |

Consequence: `create_effect` returns an `Effect` handle, marked `#[must_use]`. Drop it and the effect stops. (JS has no equivalent — Ryan's comments admit the simple JS version never disposes effects. Rust forces us to solve it, which is a good thing.)

### 2.4 Heterogeneous signals → a trait object

A computation may read a `Signal<i32>` and a `Signal<String>`. To keep them in one `Vec`, we erase the type behind a tiny trait:

```rust
trait Source {
    fn unsubscribe(&self, id: ComputationId);
}
```

This is the Rust version of the JS `running.dependencies` set (which, as a commenter noticed, is really a "set of subscriber sets").

### 2.5 Identity → numeric ids

JS uses object identity in a `Set`. We give each computation a `u64` id and store subscribers in a `BTreeMap<id, Weak<Computation>>`. `BTreeMap` (not `HashMap`) keeps iteration **in creation order**, which gives deterministic effect ordering and — as we'll see in Step 10 — glitch-free memos.

---

## Step 1 — Project setup

```bash
cargo new --lib signals
cd signals
```

No dependencies. Everything goes in `src/lib.rs`. Start it with the imports we'll need:

```rust
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::{Rc, Weak};
```

---

## Step 2 — A plain (non-reactive) signal

First, the article's opening move: a signal is just a getter/setter pair around a shared value.

```rust
// Temporary version — will be replaced in Step 5.
struct SignalInner<T> {
    value: RefCell<T>,
}

pub struct ReadSignal<T>  { inner: Rc<SignalInner<T>> }
pub struct WriteSignal<T> { inner: Rc<SignalInner<T>> }

pub fn create_signal<T>(value: T) -> (ReadSignal<T>, WriteSignal<T>) {
    let inner = Rc::new(SignalInner { value: RefCell::new(value) });
    (ReadSignal { inner: inner.clone() }, WriteSignal { inner })
}

impl<T: Clone> ReadSignal<T> {
    pub fn get(&self) -> T { self.inner.value.borrow().clone() }
}
impl<T> WriteSignal<T> {
    pub fn set(&self, v: T) { *self.inner.value.borrow_mut() = v; }
}
```

```rust
let (count, set_count) = create_signal(3);
println!("{}", count.get());      // 3
set_count.set(5);
set_count.set(count.get() * 2);
println!("{}", count.get());      // 10
```

**Why split read/write?** Same reason as Solid: you can hand a component only the `ReadSignal` and it *cannot* write. Rust's type system makes this capability split free.

Implement `Clone` by hand (a derive would wrongly require `T: Clone`):

```rust
impl<T> Clone for ReadSignal<T> {
    fn clone(&self) -> Self { Self { inner: self.inner.clone() } }
}
impl<T> Clone for WriteSignal<T> {
    fn clone(&self) -> Self { Self { inner: self.inner.clone() } }
}
```

Nothing is reactive yet. On to the machinery.

---

## Step 3 — The observer context stack

The JS:

```js
const context = [];
```

The Rust:

```rust
type ComputationId = u64;

thread_local! {
    /// The `context` stack. `None` entries mean "untracked" (used by `untrack`, Step 9).
    static OBSERVER: RefCell<Vec<Option<Rc<Computation>>>> = const { RefCell::new(Vec::new()) };
    static NEXT_ID: Cell<ComputationId> = const { Cell::new(0) };
}

fn next_id() -> ComputationId {
    NEXT_ID.with(|n| {
        let id = n.get();
        n.set(id + 1);
        id
    })
}

/// `context[context.length - 1]`
fn current_observer() -> Option<Rc<Computation>> {
    OBSERVER.with(|o| o.borrow().last().cloned().flatten())
}
```

Pushing and popping must be balanced **even if the user's function panics** — the JS uses `try { fn() } finally { context.pop() }`. In Rust, the idiom for `finally` is a guard whose `Drop` does the cleanup:

```rust
/// Push `obs`, run `f`, pop again — even if `f` panics.
fn with_observer<R>(obs: Option<Rc<Computation>>, f: impl FnOnce() -> R) -> R {
    struct PopGuard;
    impl Drop for PopGuard {
        fn drop(&mut self) {
            OBSERVER.with(|o| { o.borrow_mut().pop(); });
        }
    }
    OBSERVER.with(|o| o.borrow_mut().push(obs));
    let _guard = PopGuard;
    f()
}
```

**Why a stack and not a single variable?** Nested computations. If effect A creates effect B while running, then after B finishes, reads in A must still be attributed to A. Ryan's example from the comments — reading `name()` *after* creating an inner effect — fails with a single variable. A stack restores the outer observer automatically. (There's a test for exactly this in Step 12.)

---

## Step 4 — The `Computation` (the "running" object)

In JS:

```js
const running = { execute, dependencies: new Set() };
```

In Rust:

```rust
trait Source {
    fn unsubscribe(&self, id: ComputationId);
}

struct Computation {
    id: ComputationId,
    /// The user's function. RefCell because running it needs `&mut` (FnMut).
    f: RefCell<Box<dyn FnMut()>>,
    /// Back-links: every signal read during the last run.
    sources: RefCell<Vec<Rc<dyn Source>>>,
    /// Callbacks registered with `on_cleanup` during the last run (Step 7).
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
}
```

Now `execute()`. The JS:

```js
const execute = () => {
  cleanup(running);
  context.push(running);
  try { fn(); } finally { context.pop(); }
};
```

The Rust:

```rust
impl Computation {
    fn run(self: &Rc<Self>) {
        // Re-entrancy guard (explained in Step 11).
        let Ok(mut f) = self.f.try_borrow_mut() else { return; };
        self.cleanup();
        with_observer(Some(self.clone()), &mut *f);
    }

    /// `cleanup(running)` from the article.
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
}
```

Two Rust details worth noticing:

- `self: &Rc<Self>` — a method receiver that gives us the `Rc` itself, so we can push a clone of it onto the context stack.
- `std::mem::take` — we move the `Vec` *out* of the `RefCell` before iterating. If we iterated while holding `borrow_mut()`, any callback that touched `sources` would panic with `BorrowMutError`. **Take, then iterate** is the general rule for callbacks inside `RefCell`s.

---

## Step 5 — Make signals reactive: `track` and `notify`

Upgrade `SignalInner` with a subscriber list:

```rust
struct SignalInner<T> {
    value: RefCell<T>,
    /// Ordered by id → effects re-run in creation order (deterministic).
    subscribers: RefCell<BTreeMap<ComputationId, Weak<Computation>>>,
}

impl<T> Source for SignalInner<T> {
    fn unsubscribe(&self, id: ComputationId) {
        self.subscribers.borrow_mut().remove(&id);
    }
}
```

### 5.1 `track` — the JS `subscribe(running, subscriptions)`

```js
function subscribe(running, subscriptions) {
  subscriptions.add(running);
  running.dependencies.add(subscriptions);
}
```

```rust
impl<T: 'static> SignalInner<T> {
    fn track(self: &Rc<Self>) {
        if let Some(running) = current_observer() {
            let newly_added = self
                .subscribers
                .borrow_mut()
                .insert(running.id, Rc::downgrade(&running))
                .is_none();
            if newly_added {
                // the back-link, type-erased to `dyn Source`
                running.sources.borrow_mut().push(self.clone() as Rc<dyn Source>);
            }
        }
    }
}
```

The `newly_added` check means reading the same signal ten times in one run records it once.

### 5.2 `notify` — the JS `write`

```js
for (const sub of [...subscriptions]) sub.execute();
```

That `[...subscriptions]` copy matters (Ryan explains it in the comments): each subscriber removes itself and re-adds itself while running, so iterating the live set loops forever. In Rust the borrow checker would stop us anyway — we'd be mutating the map while borrowed. So we **snapshot**:

```rust
// First version — upgraded in Step 10.
fn notify(&self) {
    let subs: Vec<Rc<Computation>> = {
        let mut map = self.subscribers.borrow_mut();
        map.retain(|_, w| w.strong_count() > 0);          // drop dead (disposed) effects
        map.values().filter_map(Weak::upgrade).collect()  // snapshot as strong refs
    };                                                    // borrow released here
    for sub in subs {
        sub.run();
    }
}
```

### 5.3 The public read/write API

```rust
pub fn create_signal<T: 'static>(value: T) -> (ReadSignal<T>, WriteSignal<T>) {
    let inner = Rc::new(SignalInner {
        value: RefCell::new(value),
        subscribers: RefCell::new(BTreeMap::new()),
    });
    (ReadSignal { inner: inner.clone() }, WriteSignal { inner })
}

impl<T: 'static> ReadSignal<T> {
    /// Tracked read by reference — no clone, works for non-Clone T.
    pub fn with<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        self.inner.track();
        f(&self.inner.value.borrow())
    }
    /// Tracked read that clones.
    pub fn get(&self) -> T where T: Clone {
        self.with(T::clone)
    }
}

impl<T: 'static> WriteSignal<T> {
    pub fn set(&self, value: T) {
        *self.inner.value.borrow_mut() = value;   // borrow ends at the `;`
        self.inner.notify();                      // …so effects can read the new value
    }
    /// Mutate in place — ideal for Vec / HashMap signals.
    pub fn update(&self, f: impl FnOnce(&mut T)) {
        f(&mut self.inner.value.borrow_mut());
        self.inner.notify();
    }
}
```

`with` vs `get` is a Rust-specific addition: `get` clones (cheap for `i32`, costly for a large `Vec`), `with` lends a reference. This is the same split Leptos uses.

> ⚠️ The order in `set` is important: release the `borrow_mut()` **before** calling `notify()`. Otherwise the first effect that reads the signal panics.

---

## Step 6 — Effects with ownership

```js
export function createEffect(fn) {
  const running = { execute, dependencies: new Set() };
  execute();
}
```

```rust
/// Owning handle to a running effect. Dropping it disposes the effect.
#[must_use = "dropping an Effect immediately disposes it"]
pub struct Effect {
    _computation: Rc<Computation>,
}

impl Effect {
    /// Stop the effect now (same as dropping it).
    pub fn dispose(self) {}
    /// Keep it alive for the rest of the program (JS-style "fire and forget").
    pub fn leak(self) { std::mem::forget(self); }
}

pub fn create_effect(f: impl FnMut() + 'static) -> Effect {
    let computation = Computation::new(f);
    computation.run();                // run once immediately → collects dependencies
    Effect { _computation: computation }
}
```

The article's first demo now works:

```rust
println!("1. Create Signal");
let (count, set_count) = create_signal(0);

println!("2. Create Reaction");
let _e = create_effect(move || println!("The count is {}", count.get()));

println!("3. Set count to 5");
set_count.set(5);

println!("4. Set count to 10");
set_count.set(10);
```

```
1. Create Signal
2. Create Reaction
The count is 0
3. Set count to 5
The count is 5
4. Set count to 10
The count is 10
```

Note `let _e = …` — **not** `let _ = …`. `_` drops immediately (effect dies); `_e` keeps it until end of scope. The `#[must_use]` attribute makes the compiler warn you if you forget.

---

## Step 7 — Dynamic dependencies, cleanup and `on_cleanup`

Because `run()` calls `cleanup()` first, every run starts with zero dependencies and re-collects exactly what it reads *this time*. Conditional reads therefore subscribe and unsubscribe automatically:

```rust
let display = create_memo(move || {          // memo arrives in Step 8
    if !show_full.get() {
        return first.get();                   // `last` is NOT read on this branch…
    }
    format!("{} {}", first.get(), last.get())
});
```

After `set_show_full(false)`, nobody is subscribed to `last`, so `set_last("Legend")` triggers nothing. As the article puts it, this is safe: for `last` to matter again, `show_full` must change — and that *is* tracked.

### `on_cleanup`

User-level teardown (clear a timer, close a socket, dispose a child effect) hooks into the same moment:

```rust
pub fn on_cleanup(f: impl FnOnce() + 'static) {
    if let Some(running) = current_observer() {
        running.cleanups.borrow_mut().push(Box::new(f));
    }
}
```

Cleanups run **before the next run** (not after the current one), exactly as Ryan explains: `run 1 → cleanup 1 → run 2 → cleanup 2 → … → final cleanup on dispose`. The dependencies must stay in place between runs — that's how the effect gets notified at all.

### Disposing: `impl Drop`

When the last `Rc<Computation>` goes away (the user dropped the `Effect`), unsubscribe and run pending cleanups:

```rust
impl Drop for Computation {
    fn drop(&mut self) {
        self.cleanup();
    }
}
```

### Nested effects without zombies

A reader of Ryan's article found that nested effects leak "zombie" subscriptions in the JS version, because the inner effect is recreated on every outer run and never disposed. In Rust the solution falls out of ownership — make the outer run *own* the inner effect:

```rust
let _outer = create_effect(move || {
    let value = value.clone();
    let inner = create_effect(move || println!("value {}", value.get()));
    on_cleanup(move || drop(inner));        // disposed when the outer re-runs
    println!("name {}", name.get());        // still tracked by OUTER (context stack!)
});
```

This is a manual version of Solid's "owner tree". See [Where to go next](#where-to-go-next).

---

## Step 8 — Memos (derivations)

Ryan's minimal version:

```js
export function createMemo(fn) {
  const [s, set] = createSignal();
  createEffect(() => set(fn()));
  return s;
}
```

Rust has no `undefined`, so we can't create the signal before we know the first value. Solution: create the signal *inside* the first run of the effect and stash it in a shared slot.

We also add the most useful memo feature: **only notify when the value actually changed** (`T: PartialEq`). An `is_even` memo over a counter then stops downstream work on every odd→odd change.

```rust
/// A cached derived value. Trackable, like a signal.
pub struct Memo<T> {
    read: ReadSignal<T>,
    _effect: Rc<Effect>,     // the memo owns its internal effect; Rc makes Memo cheap to Clone
}

impl<T> Clone for Memo<T> {
    fn clone(&self) -> Self {
        Self { read: self.read.clone(), _effect: self._effect.clone() }
    }
}

pub fn create_memo<T: PartialEq + 'static>(mut f: impl FnMut() -> T + 'static) -> Memo<T> {
    type Slot<T> = Rc<RefCell<Option<(ReadSignal<T>, WriteSignal<T>)>>>;
    let slot: Slot<T> = Rc::new(RefCell::new(None));

    let slot_in_effect = slot.clone();
    let effect = create_effect(move || {
        let next = f();                                   // tracked
        let existing = slot_in_effect.borrow().clone();   // clone out → release borrow
        match existing {
            None => *slot_in_effect.borrow_mut() = Some(create_signal(next)),  // first run
            Some((read, write)) => {
                if read.with_untracked(|old| *old != next) {  // equality check
                    write.set(next);
                }
            }
        }
    });

    let (read, _write) = slot.borrow().clone().expect("memo ran once on creation");
    Memo { read, _effect: Rc::new(effect) }
}

impl<T: 'static> Memo<T> {
    pub fn with<R>(&self, f: impl FnOnce(&T) -> R) -> R { self.read.with(f) }
    pub fn get(&self) -> T where T: Clone { self.read.get() }
    pub fn get_untracked(&self) -> T where T: Clone { self.read.get_untracked() }
}
```

We need two untracked read helpers on `ReadSignal` (used above, and by `untrack`-style code):

```rust
impl<T: 'static> ReadSignal<T> {
    pub fn with_untracked<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        f(&self.inner.value.borrow())
    }
    pub fn get_untracked(&self) -> T where T: Clone {
        self.with_untracked(T::clone)
    }
}
```

Why the comparison must be *untracked*: reading `old` with `.with()` would subscribe the memo to **its own** output signal → it would re-run whenever it writes itself.

**Memo vs plain closure.** As Ryan says in the comments, don't over-use memos. `let full = move || format!(…)` is a perfectly good derived value when only one place reads it. Reach for `create_memo` when (a) the computation is expensive, (b) many readers share it, or (c) you want the equality cut-off.

---

## Step 9 — `untrack`

Sometimes an effect needs a value but must *not* re-run when it changes (e.g. reading a config while reacting to a click count). We already made the context stack hold `Option`s — push `None`:

```rust
pub fn untrack<R>(f: impl FnOnce() -> R) -> R {
    with_observer(None, f)
}
```

```rust
let _e = create_effect(move || {
    let b_val = untrack(|| b.get());   // read, but no subscription
    println!("{}", a.get() + b_val);   // re-runs on `a` only
});
```

---

## Step 10 — `batch` and glitch-free updates

### 10.1 The problem

With the Step 5 `notify`, updates run **immediately and depth-first**. Two symptoms:

**(a) Multiple writes → multiple runs.**

```rust
set_a.set(2);   // effect runs
set_b.set(3);   // effect runs again
```

**(b) Glitches — observing inconsistent state.** The classic diamond:

```
         a
       ↙   ↘
   double  triple      (memos)
       ↘   ↙
       effect: println!("{} {}", double, triple)
```

`set_a(2)` runs `double` first → it writes → the effect runs *immediately* with the new `double` and the **old** `triple`, printing `4 3` — a state that never logically existed. Then `triple` updates and the effect prints `4 6`.

### 10.2 The fix: a queue, flushed in creation order

Add two pieces of runtime state:

```rust
thread_local! {
    // …existing entries…
    static BATCH_DEPTH: Cell<usize> = const { Cell::new(0) };
    /// Computations waiting to run, ordered by id (= creation order).
    static PENDING: RefCell<BTreeMap<ComputationId, Weak<Computation>>> =
        const { RefCell::new(BTreeMap::new()) };
}
```

Computations are now **scheduled**, not run directly:

```rust
impl Computation {
    fn is_running(&self) -> bool {
        self.f.try_borrow_mut().is_err()
    }

    fn schedule(self: &Rc<Self>) {
        if self.is_running() {
            return;                        // Step 11
        }
        if BATCH_DEPTH.with(|d| d.get()) > 0 {
            // Keyed by id → duplicates collapse, and the queue stays sorted.
            PENDING.with(|p| { p.borrow_mut().insert(self.id, Rc::downgrade(self)); });
        } else {
            self.run();
        }
    }
}
```

`batch` increments the depth, runs the user's closure, and the **outermost** batch flushes:

```rust
pub fn batch<R>(f: impl FnOnce() -> R) -> R {
    BATCH_DEPTH.with(|d| d.set(d.get() + 1));
    let result = f();

    if BATCH_DEPTH.with(|d| d.get()) == 1 {
        // Stay at depth 1 while flushing, so writes made BY effects are queued too.
        // Always run the OLDEST pending computation next.
        while let Some((_, weak)) = PENDING.with(|p| p.borrow_mut().pop_first()) {
            if let Some(c) = weak.upgrade() {
                c.run();
            }
        }
    }

    BATCH_DEPTH.with(|d| d.set(d.get() - 1));
    result
}
```

Finally, upgrade `notify` so **every write is its own mini-transaction**:

```rust
fn notify(&self) {
    let subs: Vec<Rc<Computation>> = {
        let mut map = self.subscribers.borrow_mut();
        map.retain(|_, w| w.strong_count() > 0);
        map.values().filter_map(Weak::upgrade).collect()
    };
    batch(|| {
        for sub in subs {
            sub.schedule();
        }
    });
}
```

### 10.3 Why creation order works

It's the key insight, and I learned it the hard way while testing this guide. The first version of the flush was a plain FIFO. Running the article's batch example:

```rust
let c = create_memo(move || b.get() * 2);
let _e = create_effect(move || println!("sum {}", a.get() + c.get()));
batch(|| { set_a.set(2); set_b.set(3); });
```

FIFO printed `sum 5, sum 6, sum 8`. `set_a` queued the **effect** before `set_b` queued the **memo**, so the effect ran with a stale `c` (`sum 6`), then ran again. Exactly the problem Ryan describes: "you have to start somewhere".

The fix: a memo can only be read by something that already has a handle to it, so **a memo is always created before its readers and has a smaller id**. Popping the smallest id first means upstream memos settle before downstream effects run. Since `PENDING` is keyed by id, when the memo re-runs and schedules the effect again, the entry collapses into the one already queued — the effect runs **once**, with consistent values:

```
sum 5
sum 8
```

and the diamond prints `2 3` then `4 6` — no glitch.

---

## Step 11 — Safety nets: re-entrancy, panics, `Drop`

Ryan's conclusion lists what his 50-line version lacks: batching, disposal, **safeguards against infinite recursion**, glitch-freedom. We've covered three; here's the fourth, plus two Rust-specific traps.

### 11.1 An effect that writes to what it reads

```rust
let _e = create_effect(move || {
    let v = n.get();
    if v < 100 { set_n.set(v + 1); }   // writes its own dependency
});
```

Naively: run → set → notify → run → set → … stack overflow. We have two guards, both based on the fact that **while an effect runs, its `f` is mutably borrowed**:

- `run()` uses `try_borrow_mut()` and returns early if it's already borrowed.
- `schedule()` skips a computation that `is_running()`, so it doesn't get re-queued during a flush either.

Result: the effect runs once; `n` ends at `1`. That's a policy choice (Solid behaves similarly). Two *different* effects writing each other's inputs can still ping-pong forever — real libraries add an iteration limit with an error; that's a good exercise.

### 11.2 Panics inside effects

`with_observer` pops the context via a `Drop` guard, so a panic in user code can't leave a stale observer on the stack that would silently capture unrelated reads later.

### 11.3 `RefCell` borrow rules — the one real footgun

```rust
count.with(|v| set_count.set(*v + 1));   // 💥 BorrowMutError
```

`with` holds a shared borrow of the value while your closure runs; `set` needs a mutable one. Use `get()` (clone out first) or `update()` instead. Every internal method in this crate follows "**release borrows before calling user code**" (`mem::take`, snapshotting subscribers, cloning out of the memo slot) for exactly this reason.

---

## Step 12 — Tests

These live in `#[cfg(test)] mod tests` at the bottom of `lib.rs` (full listing in the appendix). A small helper collects log lines:

```rust
fn log() -> (Rc<RefCell<Vec<String>>>, impl Fn(String) + Clone) {
    let l = Rc::new(RefCell::new(Vec::new()));
    let l2 = l.clone();
    (l, move |s: String| l2.borrow_mut().push(s))
}
```

What each test proves:

| Test                                      | Verifies                                                        |
|-------------------------------------------|-----------------------------------------------------------------|
| `signal_get_set`                          | Step 2 basics, `update`                                         |
| `effect_reruns_on_change`                 | Article demo #1                                                 |
| `memo_computes_once_for_many_readers`     | Memo caches: 2 effects, 1 computation per change                |
| `dynamic_dependencies`                    | Article's `showFullName` example: `set_last` is ignored while hidden |
| `on_cleanup_runs_before_rerun_and_on_dispose` | Cleanup ordering + `Drop` disposal                          |
| `untrack_does_not_subscribe`              | Step 9                                                          |
| `batch_runs_effect_once`                  | Article's batch example → `sum 5`, `sum 8` (no `sum 6`)         |
| `diamond_is_glitch_free`                  | No inconsistent `4 3`                                           |
| `memo_skips_equal_values`                 | `PartialEq` cut-off                                             |
| `self_write_does_not_loop`                | Re-entrancy guard                                               |
| `nested_effects_track_correctly`          | Context stack restores the outer observer                       |

```bash
$ cargo test
running 11 tests
test tests::batch_runs_effect_once ... ok
test tests::diamond_is_glitch_free ... ok
test tests::dynamic_dependencies ... ok
test tests::effect_reruns_on_change ... ok
test tests::memo_computes_once_for_many_readers ... ok
test tests::memo_skips_equal_values ... ok
test tests::nested_effects_track_correctly ... ok
test tests::on_cleanup_runs_before_rerun_and_on_dispose ... ok
test tests::self_write_does_not_loop ... ok
test tests::signal_get_set ... ok
test tests::untrack_does_not_subscribe ... ok

test result: ok. 11 passed; 0 failed
```

---

## Step 13 — Run the article's demo

`examples/demo.rs` — the `onCleanup` example from the first article, ported:

```rust
use signals::*;

fn main() {
    println!("1. Create");
    let (first, _set_first) = create_signal("John".to_string());
    let (last, set_last) = create_signal("Smith".to_string());
    let (show_full, set_show_full) = create_signal(true);

    let display = create_memo(move || {
        println!("   ### executing display_name");
        on_cleanup(|| println!("   ### releasing display_name dependencies"));
        if !show_full.get() {
            return first.get();
        }
        format!("{} {}", first.get(), last.get())
    });

    let _effect = create_effect(move || println!("My name is {}", display.get()));

    println!("2. Set show_full: false");
    set_show_full.set(false);
    println!("3. Change last name");
    set_last.set("Legend".into());
    println!("4. Set show_full: true");
    set_show_full.set(true);
}
```

```bash
$ cargo run --example demo
1. Create
   ### executing display_name
My name is John Smith
2. Set show_full: false
   ### releasing display_name dependencies
   ### executing display_name
My name is John
3. Change last name
4. Set show_full: true
   ### releasing display_name dependencies
   ### executing display_name
My name is John Legend
   ### releasing display_name dependencies
```

Step 3 prints nothing — `last` has no subscribers at that moment. The final "releasing" line is the memo being **disposed** when `main` ends (the `Drop` impl at work — something the JS version never does).

---

## Pitfalls & limitations

1. **Clone before `move`.** `ReadSignal` is `Clone` but not `Copy`, so using one signal in two closures needs `let a2 = a.clone();`. Leptos avoids this with `Copy` handles into an arena (see below).
2. **`with` + `set` on the same signal panics** (Step 11.3).
3. **Single-threaded.** `Rc`/`RefCell` are `!Send`. For multi-threaded use you'd swap to `Arc` + `Mutex`/`RwLock` (or `parking_lot`), and the observer stack stays thread-local. Expect lock-ordering complexity.
4. **Creation order ≈ topological order — mostly.** If an *older* effect reads a memo created *later* (e.g. a memo built inside another effect), the older effect can run before that memo refreshes. Production libraries fix this properly with **push-pull** evaluation: writes only mark nodes `Dirty`/`Check`, and a memo recomputes lazily when read (Solid 2, Preact Signals, Leptos `reactive_graph`, the TC39 signals proposal, MobX's algorithm linked in the article).
5. **Reading a memo inside `batch` can be stale**, since its re-computation is queued until the batch ends (the same thing a commenter observed in Solid 1).
6. **Effects are eager and synchronous.** Every write re-runs subscribers before `set` returns. For UIs you often want effects deferred to a microtask/frame; add a scheduler that drains `PENDING` later instead of at the end of `batch`.
7. **Ping-pong loops between two effects are not detected.** Add a max-iterations counter to the flush loop.

---

## Where to go next

- **Owner tree (automatic disposal).** Instead of manual `on_cleanup(move || drop(inner))`, keep an `OWNER` stack alongside `OBSERVER`. Every effect/memo created while an owner runs registers as its child; `cleanup()` disposes children first. This is Solid's `createRoot` / ownership model, explained in Ryan's follow-up *SolidJS: Reactivity to Rendering*.
- **Arena + `Copy` handles.** Store all nodes in a `slotmap::SlotMap` inside the runtime and make `ReadSignal<T>` a `Copy` key + `PhantomData<T>`. No more `.clone()` before `move` — this is how Leptos 0.1–0.6 and Sycamore work.
- **Push-pull with coloring.** Replace immediate re-runs with states `Clean | Check | Dirty`; propagate `Check` down, then pull and recompute only what's needed. Gives true glitch-freedom for any graph shape and lazy memos.
- **Rendering.** A DOM text node updated by an effect is the whole idea behind fine-grained UI frameworks: `create_effect(move || text_node.set_data(&count.get().to_string()))` — the component function runs once; only effects re-run.
- **Study real implementations:** `leptos`/`reactive_graph`, `sycamore-reactive`, `futures-signals` (a stream-based alternative), and Solid's `packages/solid/src/reactive/signal.ts`.

---

## Appendix — Full source of `src/lib.rs`

This is the exact file that produced the test and demo output above.

```rust
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
```
