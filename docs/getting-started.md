# Getting started

Everything here runs. The snippets are compiled as tests by `cargo test`.

## 1. A signal is a value that remembers who read it

```rust
use rok_ui_hooks::*;

let (count, set_count) = use_state(0);
assert_eq!(count.get(), 0);

set_count.set(1);
assert_eq!(count.get(), 1);
```

`use_state` hands you two halves. The `ReadSignal` is what you read; the
`WriteSignal` is what you write. Both are cheap handles to the same cell, so
passing one around never copies your data.

A write is not a function call with side effects bolted on — it bumps a version
and marks whoever read the value. Nothing runs yet. That gap is what makes the
rest of the design possible.

```rust
use rok_ui_hooks::*;

let (name, set_name) = use_state(String::from("ada"));

// `update` works in place, so you never need `clone()` at the call site.
set_name.update(|name| name.push_str(" lovelace"));
assert_eq!(name.get(), "ada lovelace");
```

## 2. Effects run when what they read changes

An effect is a closure that runs now, and again whenever a signal it read
changes. Read inside the body to subscribe — that is the whole mechanism.

```rust
use rok_ui_hooks::*;

let (celsius, set_celsius) = use_state(20i32);
let _watch = use_effect(
    { let celsius = celsius.clone(); move || println!("{}°C", celsius.get()) },
    (),
);

set_celsius.set(21); // 21°C
set_celsius.set(22); // 22°C
```

Two details matter:

- **The closure must own its signals.** `Rc` handles are not `Copy`, and `'static`
  means "no borrowed locals". Clone into the closure; that is the cost of a
  subscription.
- **Writes run the effect after the batch, not during it**, so an effect that
  writes another signal settles instead of recursing.

## 3. Memos are values that recompute on demand

```rust
use rok_ui_hooks::*;

let (width, set_width) = use_state(3u32);
let area = use_memo(
    { let width = width.clone(); move || width.get() * width.get() },
    (),
);

assert_eq!(area.get(), 9);
set_width.set(10);
assert_eq!(area.get(), 100);
```

A memo is lazy: writing `width` does not recompute `area`, it only marks it.
`area` recomputes when it is read — once, no matter how many readers there are.
If the new value equals the old one, its readers are not woken at all. That is
the property that keeps a wide graph cheap.

### Custom equality

`use_memo` bails out with `PartialEq`. When a value is *not* cheaply comparable,
say so explicitly — the comparator decides whether downstream work happens at
all.

```rust
use rok_ui_hooks::*;
use std::rc::Rc;

let (items, set_items) = use_state(vec![1, 2, 3]);
let label = use_memo_eq(
    { let items = items.clone(); move || Rc::new(format!("{:?}", items.get())) },
    (),
    // Same text, new allocation: no downstream effect should run.
    |next: &Rc<String>, current: &Rc<String>| next == current,
);

let runs = std::rc::Rc::new(std::cell::Cell::new(0));
let _effect = use_effect(
    {
        let (label, runs) = (label.clone(), runs.clone());
        move || {
            let _ = label.get();
            runs.set(runs.get() + 1);
        }
    },
    (),
);

set_items.set(vec![1, 2, 3]); // equal text: the memo holds
assert_eq!(runs.get(), 1, "the effect did not re-run");

set_items.set(vec![1, 2, 4]); // different text: it propagates
assert_eq!(runs.get(), 2);
```

### Shallow collection equality

Allocating a fresh `Vec` every time a signal changes defeats `PartialEq`. Compare
the contents instead:

```rust
use rok_ui_hooks::*;

let (rows, set_rows) = use_state(vec![1, 2, 3]);
let total = use_memo_eq(
    { let rows = rows.clone(); move || rows.get() },
    (),
    shallow_vec_eq,
);
assert_eq!(total.get(), vec![1, 2, 3]);
```

## 4. Scopes own what is created inside them

Without ownership, an effect outlives the state it reads and the graph keeps
growing. Every hook here belongs to the scope that created it.

```rust
use rok_ui_hooks::*;

let (n, set_n) = use_state(0);
let runs = std::rc::Rc::new(std::cell::Cell::new(0));

{
    let _scope = Root::new();
    _scope.run(|| {
        let (n, runs) = (n.clone(), runs.clone());
        let _effect = use_effect(move || {
            let _ = n.get();
            runs.set(runs.get() + 1);
        }, ());
    });
    set_n.set(1); // the scope is still alive
    assert_eq!(runs.get(), 2);
}

set_n.set(2); // disposed: nothing runs
assert_eq!(runs.get(), 2);
```

`create_root(|| …)` is the same thing for a scope that ends when the closure
returns. See the ownership section of [`concepts.md`](concepts.md#ownership) — or
just run `examples/ownership.rs`.

## 5. Batching, untracking, flushing

```rust
use rok_ui_hooks::*;

let (a, set_a) = use_state(0);
let (b, set_b) = use_state(0);
let runs = std::rc::Rc::new(std::cell::Cell::new(0));

let _effect = use_effect(
    {
        let (a, b, runs) = (a.clone(), b.clone(), runs.clone());
        move || {
            let _ = (a.get(), b.get());
            runs.set(runs.get() + 1);
        }
    },
    (),
);

let after_mount = runs.get();
batch(|| {
    set_a.set(1);
    set_b.set(1);
});
assert_eq!(runs.get(), after_mount + 1, "one effect run for two writes");

// A read outside any computation is never tracked, so this cannot loop.
let total = untrack(|| a.get() + b.get());
assert_eq!(total, 2);
```

`flush()` runs whatever is pending right now; `tick()` adds timers and futures to
that and is what a UI loop calls once per frame.

## 6. Where to go next

- [`concepts.md`](concepts.md) — the model behind the API.
- [`api-reference.md`](api-reference.md) — every item, grouped.
- [`async-and-time.md`](async-and-time.md) — resources, futures, timers.
- [`patterns.md`](patterns.md) — reducers, keyed lists, context, pitfalls.
- `examples/demo.rs` and `examples/advanced.rs` for the long form.