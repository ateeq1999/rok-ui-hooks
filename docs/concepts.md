# Concepts

## Three kinds of node

**Signals** hold a value. They have no dependencies and no behaviour — a write
bumps a version counter and marks the computations that read them.

**Memos** are derived values. They are lazy and cached: a marked memo recomputes
when somebody reads it, and if the new value compares equal to the old one, its
readers are never woken.

**Effects** are side effects. They are eager: a marked effect is queued and runs
when the enclosing batch ends.

Push (signals mark) and pull (memos compute) is the whole trick. Reads are
cheap, writes are proportional to the number of subscribers, and the most
expensive node type is the one that cannot be skipped.

## States and why they exist

Every node is in one of three states:

| state | meaning |
|-------|---------|
| `Clean` | up to date |
| `Check` | something it depends on was written, but the value may be unchanged |
| `Dirty` | something it depends on was written, and it must recompute |

The middle state is the one people skip. A signal that always reports "changed"
turns a diamond into a cascade: `a` feeds `b` and `c`, both feed `d`, and `d`
runs even though `b + c` came out the same. With states, `b` and `c` each go
`Clean → Check`, recompute on read, find themselves equal, and never mark `d`.

## Tracking is a read, not a subscription call

You do not call `subscribe`. Reading a signal while a computation is running is
what records the edge:

```rust
use signals::*;

let (a, set_a) = use_state(1);
let (b, set_b) = use_state(10);

let total = use_memo(
    { let (a, b) = (a.clone(), b.clone()); move || a.get() + b.get() },
    (),
);

set_a.set(2);
set_b.set(20);
assert_eq!(total.get(), 22, "recomputed once, on read");
```

Dependencies are **dynamic**: whatever the body actually read on the last run is
what it is subscribed to. Take a branch and the dependency disappears; take
another and it appears.

```rust
use signals::*;

let (toggle, set_toggle) = use_state(false);
let (name, set_name) = use_state(String::from("ada"));
let (doubled, runs) = (use_ref(0i32), use_ref(0));

let _effect = use_effect(
    {
        let (toggle, name, doubled, runs) =
            (toggle.clone(), name.clone(), doubled.clone(), runs.clone());
        move || {
            *runs.borrow_mut() += 1;
            if toggle.get() {
                *doubled.borrow_mut() = name.get().len() as i32 * 2;
            }
        }
    },
    (),
);

set_name.set("grace".into());
assert_eq!(*runs.borrow(), 1, "it was not subscribed to `name` yet");

batch(|| {
    set_toggle.set(true);
    set_name.set("hopper".into());
});
assert_eq!(*runs.borrow(), 2, "one run for two writes");
assert_eq!(*doubled.borrow(), 12, "and it reads the new name");
```

## Batching is the flush boundary

A write never runs a subscriber synchronously. It queues the subscriber. When
the outermost `batch` (or the outermost write, if you never batched) ends, the
queue drains in creation order.

Creation order matters: a memo created before an effect is refreshed before that
effect runs, so the effect reads a consistent graph — no intermediate state, no
glitch.

```rust
use signals::*;

let (a, set_a) = use_state(1);
let doubled = use_memo(
    { let a = a.clone(); move || a.get() * 2 },
    (),
);
let seen = use_ref(0);
let runs = use_ref(0);

let _effect = use_effect(
    {
        let (doubled, seen, runs) = (doubled.clone(), seen.clone(), runs.clone());
        move || {
            *seen.borrow_mut() = doubled.get();
            *runs.borrow_mut() += 1;
        }
    },
    (),
);

let after_mount = *runs.borrow();
batch(|| {
    set_a.set(2);
    assert_eq!(*runs.borrow(), after_mount, "nothing has run yet");
});
assert_eq!(*runs.borrow(), after_mount + 1, "one consistent update");
assert_eq!(*seen.borrow(), 4);
```

Writes made *by* an effect are queued too, never run re-entrantly. If an effect
keeps writing to what it read, the loop guard fires with a message instead of
hanging.

## `untrack` reads without subscribing

```rust
use signals::*;

let (a, set_a) = use_state(1);
let (b, set_b) = use_state(2);

let _effect = use_effect(
    {
        let (a, b) = (a.clone(), b.clone());
        move || {
            let sum = a.get();
            // A log line that must not re-run the effect.
            println!("sum = {}", untrack(|| sum + b.get()));
        }
    },
    (),
);

set_b.set(3); // no re-run
```

## Ownership

A computation that is never disposed is a leak with extra steps. Every hook here
is owned by the scope that created it:

```rust
use signals::*;

let (n, set_n) = use_state(0);
let runs = std::rc::Rc::new(std::cell::Cell::new(0));

let root = Root::new();
root.run(|| {
    let (n, runs) = (n.clone(), runs.clone());
    let _effect = use_effect(move || {
        let _ = n.get();
        runs.set(runs.get() + 1);
    }, ());
});

let after_mount = runs.get(); // the effect ran once inside the scope
assert_eq!(after_mount, 1);

set_n.set(1);
assert_eq!(runs.get(), 2);

root.dispose();
set_n.set(2);
assert_eq!(runs.get(), 2, "the scope took its effect with it");
```

Ownership also nests: a hook created inside an effect belongs to that effect, so
a re-run disposes the previous generation before building the next. No zombies,
no "the old subscriber is still in the graph".

```rust
use signals::*;

let (label, set_label) = use_state("a".to_string());
let runs = use_ref(0);

let _effect = use_effect(
    {
        let (label, runs) = (label.clone(), runs.clone());
        move || {
            // Re-created on every run: the previous one is disposed first, so
            // there is never more than one child reading `runs`.
            let runs = runs.clone();
            let _child = use_effect(move || *runs.borrow_mut() += 1, ());
            println!("outer saw {}", label.get());
        }
    },
    (),
);

let after_mount = *runs.borrow();
set_label.set("b".into());
assert_eq!(*runs.borrow(), after_mount + 1, "one child, not two");
```

## Thread-local by design

The graph lives in `thread_local!` state. Signals, effects, memos, timers and the
executor are all `!Send` and `!Sync`, on purpose:

- `Rc` and `RefCell` cost one pointer dereference and a borrow check instead of
  an atomic.
- Nothing in your closures needs `Send`, so a closure can capture anything the
  thread owns — an `Rc<RefCell<…>>`, a file handle, a test double.
- A frame cannot be interrupted by a background write.

The cost is that a graph belongs to one thread. `ROADMAP.md` sketches what an
`Arc`-backed engine would cost, and why this crate waits for a use case rather
than a design.

## Dependencies

The `deps` argument exists for the React muscle memory and for a real reason: an
effect should not re-run because something it never looked at changed.

```rust
use signals::*;

let (a, set_a) = use_state(1);
let (b, set_b) = use_state(10);
let runs = use_ref(0);

let _effect = use_effect(
    { let (a, runs) = (a.clone(), runs.clone()); move || { let _ = a.get(); *runs.borrow_mut() += 1; } },
    (a,),
);

set_b.set(11);
assert_eq!(*runs.borrow(), 1, "deps excluded it");
set_a.set(2);
assert_eq!(*runs.borrow(), 2);
```

| deps | meaning |
|------|---------|
| `()` | re-run when anything the body read changes |
| `(count,)` | re-run only when `count` has a **new value** |
| `(0i32,)` | mount only |

The middle row is the subtle one: a handle contributes its *value*, so a memo
that recomputes to the same thing does not re-run its effect.

## Deferred effects

An eager effect runs as soon as the batch ends. A deferred one waits until the
queue is otherwise quiet — useful for work that must not interleave with the
render pass, like persisting state after a burst of edits.

```rust
use signals::*;

let (name, set_name) = use_state(String::from("ada"));
let order = use_ref(Vec::new());

let _eager = use_effect(
    { let (name, order) = (name.clone(), order.clone()); move || { let _ = name.get(); order.borrow_mut().push("eager"); } },
    (),
);
let _deferred = create_deferred_effect(
    { let (name, order) = (name.clone(), order.clone()); move || { let _ = name.get(); order.borrow_mut().push("deferred"); } },
    (),
);

order.borrow_mut().clear(); // forget the two mount runs

// Two writes, one batch: each effect is marked once.
batch(|| {
    set_name.set("grace".into());
    set_name.set("hopper".into());
});
assert_eq!(*order.borrow(), ["eager"], "the deferred one is still waiting");

flush();
assert_eq!(*order.borrow(), ["eager", "deferred"]);

// The next batch behaves the same way, after the deferred queue is quiet.
batch(|| set_name.set("lovelace".into()));
flush();
assert_eq!(
    *order.borrow(),
    ["eager", "deferred", "eager", "deferred"]
);
```

The four runs come from two writes to one signal: each write marks the effect,
each run queues the other. `flush()` drains the eager queue, and the deferred
queue after it.