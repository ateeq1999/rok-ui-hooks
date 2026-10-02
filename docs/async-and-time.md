# Async and time

There is no background thread in this crate. Futures are polled from `tick()`,
timers are drained by `tick()`, and both run **on the thread you call it from**.

That is a deliberate trade:

- Your futures and callbacks do not need `Send`. A closure can capture an
  `Rc<RefCell<…>>`, a `File`, or a test double.
- Nothing can mutate the graph from another thread, so there are no locks in the
  hot path and no cross-thread tearing.
- A frame is never interrupted by a callback firing mid-render.

The cost: **nothing async happens unless you call `tick()`** (or `flush()`, for
effects only). In a UI, that is once per frame. In a terminal program, that is a
loop you control:

```rust
use signals::*;
use std::time::Duration;

let (count, set_count) = use_state(0);
let log = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
let _effect = use_effect(
    { let (count, log) = (count.clone(), log.clone());
      move || log.borrow_mut().push(count.get()) },
    (),
);

// A frame: advance the world, then let the graph settle.
for i in 1..=3 {
    set_count.set(i);
    tick();
}
assert_eq!(*log.borrow(), [0, 1, 2, 3]);

// A loop that waits for real time: the crate stays out of the way.
let deadline = std::time::Instant::now() + Duration::from_millis(0);
while std::time::Instant::now() < deadline {
    tick();
    std::thread::sleep(Duration::from_millis(1));
}
```

## What `tick()` does, in order

1. **Timers.** Every deadline that has passed runs its callback, on this thread.
2. **Futures.** Every ready task is polled, including the ones it wakes itself,
   until the queue is empty (bounded per task, per tick).
3. **Effects.** The eager queue, then the deferred queue, in creation order.

A timer callback that writes a signal is therefore batched with everything else
in the same frame, like any other write.

## Resources

A resource is a future plus its state, with latest-wins semantics: a slow fetch
cannot overwrite a newer one, and a fetch whose scope was disposed writes
nothing.

```rust
use signals::*;

let user = use_resource(|| async { "ada" });
assert!(user.peek().is_loading(), "the fetch has not been polled yet");

tick();
assert_eq!(user.get(), ResourceState::Ready("ada"));
```

Read `state()` inside an effect to render it:

```rust
use signals::*;

let user = use_resource(|| async { 42 });
let label = use_ref(String::new());

let _effect = use_effect(
    {
        let (user, label) = (user.clone(), label.clone());
        move || {
            *label.borrow_mut() = match user.state().get() {
                ResourceState::Loading => "loading…".to_string(),
                ResourceState::Ready(value) => value.to_string(),
                ResourceState::Failed(error) => format!("error: {error}"),
                ResourceState::Idle => "nothing asked for".to_string(),
            };
        }
    },
    (),
);

assert_eq!(*label.borrow(), "loading…");
tick();
assert_eq!(*label.borrow(), "42");
```

### Cancelling and refetching

```rust
use signals::*;

// A disposed scope cancels the fetch: the sink is dropped with the scope.
{
    let (text, set_text) = use_state(String::from("before"));
    create_root(|| {
        let set_text = set_text.clone();
        let _resource = use_resource(move || {
            let set_text = set_text.clone();
            async move {
                std::future::poll_fn(move |cx| {
                    cx.waker().wake_by_ref();
                    std::task::Poll::<String>::Pending
                })
                .await
            }
        });
    });
    tick();
    assert_eq!(text.get(), "before", "the disposed fetch wrote nothing");
}

// Refetching is explicit, and it resets the state.
let profile = use_resource(|| async { 1 });
tick();
assert!(matches!(profile.peek(), ResourceState::Ready(1)));
profile.refetch();
assert!(profile.peek().is_loading());
tick();
assert!(matches!(profile.peek(), ResourceState::Ready(1)));
```

### Projecting the result

`use_resource_with` keeps the whole future's output private and stores only what
you select:

```rust
use signals::*;

struct Response { id: u32, body: String }

let id = use_resource_with(
    || async { Response { id: 7, body: "…".into() } },
    |response| response.id,
);
tick();
assert_eq!(id.peek(), ResourceState::Ready(7));
```

### A resource that follows a signal

There is no hidden `key` argument; the idiomatic way is an effect that refetches
when the input changes:

```rust
use signals::*;

let (id, set_id) = use_state(1u32);
let body = use_resource(|| async { "first" });

let _refetch = use_effect(
    {
        let (id, body) = (id.clone(), body.clone());
        move || {
            let _ = id.get();
            // Skip the mount run: the resource already fetched once.
            body.refetch();
        }
    },
    (),
);

tick();
assert!(matches!(body.peek(), ResourceState::Ready("first")));
```

## Futures

`spawn` registers a future with the local executor.

```rust
use signals::*;

let (status, set_status) = use_state(String::from("idle"));
let task = spawn({
    let set_status = set_status.clone();
    async move {
        set_status.set("done".into());
    }
});

assert!(!task.is_finished());
tick();
assert_eq!(status.get(), "done");
assert!(task.is_finished());
```

A future that is woken during a tick is polled again in the same tick, so a chain
of already-ready steps completes without waiting for a frame. A future that waits
for real I/O simply resumes on the next `tick()` that finds it woken.

### Lifetime rules

- Outside a scope: dropping the `Task` cancels the future.
- Inside a scope (`Root::run`, `create_root`, an effect, a keyed row): the scope
  owns the future, and disposing the scope cancels it. Dropping the handle does
  nothing.

```rust
use signals::*;

let polls = std::rc::Rc::new(std::cell::Cell::new(0));

let root = Root::new();
root.run(|| {
    let polls = polls.clone();
    let task = spawn(async move {
        std::future::poll_fn(move |cx| {
            polls.set(polls.get() + 1);
            cx.waker().wake_by_ref();
            std::task::Poll::<()>::Pending
        })
        .await
    });
    drop(task); // the scope owns it now
});

tick();
assert!(polls.get() > 0);

root.dispose();
let after = polls.get();
tick();
assert_eq!(polls.get(), after, "disposed: no more polls");

assert_eq!(pending_tasks(), 0);
```

### Loop protection

A future that wakes itself on every poll would spin forever inside one frame. It
is polled at most 100 times per `tick`, then left queued for the next one — the
task is slow, not dead, and nothing panics.

## Timers

`use_debounced` and `use_throttled` are the two timer-backed hooks. They hold a
deadline, and `tick()` runs the callback when the deadline passes.

### Debounce: the trailing edge

```rust
use signals::*;
use std::time::Duration;

let (text, set_text) = use_state(String::from(""));
let settled = use_debounced(&text, Duration::from_millis(10));

assert_eq!(settled.get(), "");
set_text.set("h".into());
set_text.set("he".into());
set_text.set("hello".into());

assert_eq!(settled.get(), "", "a burst keeps restarting the clock");

let mut ticks = 0;
while settled.get() != "hello" && ticks < 200 {
    std::thread::sleep(Duration::from_millis(2));
    tick();
    ticks += 1;
}
assert_eq!(settled.get(), "hello", "only the last write survives");
```

### Throttle: leading edge plus a trailing update

```rust
use signals::*;
use std::time::Duration;

let (n, set_n) = use_state(0u32);
let throttled = use_throttled(&n, Duration::from_millis(20));

set_n.set(1);
assert_eq!(throttled.get(), 1, "the first change goes straight through");

set_n.set(2);
set_n.set(3);
assert_eq!(throttled.get(), 1, "the rest wait for the window to close");

let mut ticks = 0;
while throttled.get() != 3 && ticks < 200 {
    std::thread::sleep(Duration::from_millis(2));
    tick();
    ticks += 1;
}
assert_eq!(throttled.get(), 3, "one trailing update, with the last value");
```

Both hooks return a small wrapper with `get()`, `with(f)` and `read_signal()`, so
a debounced value can be passed down the tree like any other signal.

## Building a frame loop

```rust
use signals::*;
use std::time::{Duration, Instant};

let (open, set_open) = use_state(true);
let elapsed = use_ref(Duration::ZERO);

let _effect = use_effect(
    { let (open, elapsed) = (open.clone(), elapsed.clone());
      move || if open.get() { println!("panel open") } },
    (),
);

let start = Instant::now();
set_open.set(false);
while start.elapsed() < Duration::from_millis(20) {
    *elapsed.borrow_mut() = start.elapsed();
    tick();                  // timers, futures, effects
    std::thread::sleep(Duration::from_millis(4)); // pretend this is vsync
}
assert!(*elapsed.borrow() >= Duration::from_millis(16));
```

`examples/async.rs` is this document as a program you can run.