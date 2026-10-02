# signals

Fine-grained reactivity for Rust with a React-flavoured API.

```toml
[dependencies]
signals = "0.2"
```

No dependencies, no `unsafe`, no build script, no runtime. One rule: a value is
read inside a computation, and that computation is told when the value changes.

```rust
use signals::*;

let (count, set_count) = use_state(0);

let _effect = use_effect(
    { let count = count.clone(); move || println!("count = {}", count.get()) },
    (),
);

set_count.set(1); // prints "count = 1"
```

## What it is

There is no component tree and no virtual DOM. Work is split into three kinds of
node, and the difference between them is the whole design:

| node | what it is | when it runs |
|------|------------|--------------|
| **signal** (`use_state`) | a value | a write bumps a version and *marks* its readers |
| **memo** (`use_memo`) | a derived value | lazily, when somebody reads it — and only then |
| **effect** (`use_effect`) | a side effect | eagerly, when the enclosing batch ends |

A write runs **nothing** synchronously. That is what makes any graph shape
glitch-free: an effect that reads a memo that reads two signals always sees a
consistent snapshot, no matter what order the writes arrived in.

## A tour in four hooks

```rust
use signals::*;

// 1. State.
let (name, set_name) = use_state(String::from("world"));
set_name.set("Rust".into());

// 2. Derived value, recomputed only when needed.
let greeting = use_memo(
    { let name = name.clone(); move || format!("Hello, {}!", name.get()) },
    (),
);
assert_eq!(greeting.get(), "Hello, Rust!");

// 3. Side effect, re-run when what it read changes.
let _log = use_effect(
    { let greeting = greeting.clone(); move || println!("{}", greeting.get()) },
    (greeting,),
);

// 4. Owned scope: everything created inside dies with the scope.
let root = Root::new();
root.run(|| {
    let (n, set_n) = use_state(0);
    set_n.set(1);
    assert_eq!(n.get(), 1);
}); // disposed here: no effect of `n` survives
```

## What you get

- **Signals** with tracked and untracked reads — `use_state`, `create_signal`.
- **Memos** with custom equality — `use_memo`, `use_memo_eq`,
  `shallow_vec_eq`.
- **Effects** — eager, render, deferred, with cleanup — `use_effect`,
  `create_effect`, `create_render_effect`, `create_deferred_effect`,
  `on_cleanup`.
- **Ownership** — `create_root`, `Root`, `create_owned_root`, `OwnedRoot`.
  Scopes own their computations, so a disposed scope leaks nothing.
- **Batching** — `batch`, `flush`, `tick`, `untrack`, with a loop guard that
  turns "an effect writes what it reads" into a panic with a clear message
  instead of a hang.
- **Hooks** — `use_reducer`, `use_previous`, `use_debounced`, `use_throttled`,
  `use_ref`, `use_store`.
- **Context** — `create_context`, `with_provider`, `use_context`, with
  propagation so an effect under a provider re-runs when the value changes.
- **Async** — `use_resource`, `Resource`, `spawn`, `Task`, all polled from
  `tick()` on the thread you call it from. No `Send`, no `'static` futures, no
  background thread.
- **Stores** — `create_store`, `Store::select`, `use_store`.
- **Keyed lists** — `create_keyed_list`, which keeps per-key state across
  reorders and disposes rows exactly once.

## Run the examples

```text
cargo run --example demo          # the classic walkthrough
cargo run --example advanced      # diamond deps, cleanups, glitch-free batching
cargo run --example todo_app      # a small app with a reducer and a keyed list
cargo run --example ownership     # who disposes what
cargo run --example keyed_list    # reconcile by identity, not position
cargo run --example async         # resources, futures, timers, the tick loop
cargo run --example context_demo  # provider propagation
cargo run --example hooks_demo    # reducer, previous, debounce, throttle
cargo run --example store_demo    # stores and selectors
```

## Documentation

- [`docs/getting-started.md`](docs/getting-started.md) — the guided tour.
- [`docs/concepts.md`](docs/concepts.md) — how the graph works, and why.
- [`docs/api-reference.md`](docs/api-reference.md) — every public item.
- [`docs/async-and-time.md`](docs/async-and-time.md) — resources, futures,
  timers, the tick loop.
- [`docs/patterns.md`](docs/patterns.md) — idioms and pitfalls.
- [`docs/architecture.md`](docs/architecture.md) — the internals.
- [`docs/porting.md`](docs/porting.md) — moving from the pre-0.2 API.

## Development

```text
cargo test          # unit, integration and doc tests
cargo clippy --all-targets
cargo fmt
cargo bench         # zero-dependency smoke benchmark
```

MSRV 1.85. `#![forbid(unsafe_code)]`. `#![warn(missing_docs)]`.

## License

MIT OR Apache-2.0 — your choice.