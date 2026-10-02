# API reference

Every public item, grouped by what it is for. Every example here is compiled by
`cargo test`.

## Signals

| item | what it does |
|------|--------------|
| [`use_state(initial) -> (ReadSignal<T>, WriteSignal<T>)`] | state that reads like a value and writes like a setter |
| [`create_signal(initial)`] | the same, spelled without the `use_` prefix |
| `ReadSignal::get()` | tracked read — subscribes the running computation |
| `ReadSignal::with(f)` | tracked read by reference, so a big value is not cloned |
| `ReadSignal::get_untracked()` / `with_untracked(f)` | read without subscribing |
| `ReadSignal::peek()` | a plain field access, for assertions and logs |
| `WriteSignal::set(v)` | write; marks every reader |
| `WriteSignal::update(f)` | write in place |
| `WriteSignal::get_untracked()` | read the other half (untracked) |

```rust
use rok_ui_hooks::*;

let (count, set_count) = use_state(vec![1, 2, 3]);
assert_eq!(count.get(), vec![1, 2, 3]);

set_count.update(|items| items.push(4));
assert_eq!(count.get(), vec![1, 2, 3, 4]);
assert_eq!(count.peek(), vec![1, 2, 3, 4], "no computation is running");

let doubled = count.with(|items| items.len() * 2);
assert_eq!(doubled, 8);
```

## Memos

| item | what it does |
|------|--------------|
| [`use_memo(f, deps) -> Memo<T>`] | cached derivation, bails out on `PartialEq` |
| [`use_memo_eq(f, deps, eq) -> Memo<T>`] | the same with your own equality |
| [`create_memo(f, deps)`] | alias for `use_memo` |
| [`shallow_vec_eq`] / [`shallow_array_eq`] | comparators that ignore a fresh allocation |
| `Memo::get()` | tracked read; recomputes first if needed |
| `Memo::with(f)` | tracked read by reference |
| `Memo::peek()` / `get_untracked()` / `with_untracked(f)` | read without recomputing for subscribers |
| `Memo::invalidate()` | force a recompute on the next read |
| `Memo::dispose()` | drop the memo and every edge to it |

```rust
use rok_ui_hooks::*;

let (n, set_n) = use_state(1);
let squared = use_memo({ let n = n.clone(); move || n.get() * n.get() }, ());

let runs = use_ref(0);
let _effect = use_effect(
    { let (squared, runs) = (squared.clone(), runs.clone());
      move || { let _ = squared.get(); *runs.borrow_mut() += 1; } },
    (),
);

let after_mount = *runs.borrow();
set_n.set(4);
assert_eq!(*runs.borrow(), after_mount + 1);
assert_eq!(squared.get(), 16);

squared.invalidate();
assert_eq!(squared.peek(), 16, "still the same value");
```

## Effects

| item | what it does |
|------|--------------|
| [`use_effect(f, deps) -> Effect`] | runs now, and again when what it read changes |
| [`create_effect(f, deps)`] | alias, for code outside a component-like scope |
| [`use_effect_with(setup, deps)`] | run `setup` immediately, run `f` on changes |
| [`create_render_effect(f, deps)`] | like `use_effect`, but meant for render-phase work |
| [`create_deferred_effect(f, deps)`] | runs after the eager queue is quiet |
| [`on_cleanup(f)`] / [`use_cleanup(f)`] | teardown for the current computation, else the scope |
| [`use_ref(v) -> Rc<RefCell<T>>`] | a mutable cell that is *not* reactive |
| `Effect::dispose()` / `Effect::leak()` / `Effect::is_alive()` | control the effect's life |

```rust
use rok_ui_hooks::*;

let (n, set_n) = use_state(0);
let lines = use_ref(Vec::new());

let _effect = use_effect(
    { let (n, lines) = (n.clone(), lines.clone());
      move || { lines.borrow_mut().push(n.get()); } },
    (),
);

set_n.set(1);
set_n.set(2);
lines.borrow_mut().push(999); // a ref is not tracked: no re-run
assert_eq!(*lines.borrow(), [0, 1, 2, 999]);
```

## Scope and cleanup

| item | what it does |
|------|--------------|
| [`create_root(f) -> R`] | run `f` in a scope that is disposed when `f` returns |
| [`Root::new()`] / [`Root::run(f)`] | a scope you keep and reuse |
| [`Root::dispose()`] | dispose now; safe to call twice |
| [`create_owned_root(f) -> (OwnedRoot, R)`] | a scope that outlives the call that made it |
| [`OwnedRoot::run(f)`] / [`OwnedRoot::dispose()`] | reuse or tear it down |

```rust
use rok_ui_hooks::*;

let (n, set_n) = use_state(0);
let runs = use_ref(0);

{
    let scope = Root::new();
    scope.run(|| {
        let (n, runs) = (n.clone(), runs.clone());
        let _effect = use_effect(move || { let _ = n.get(); *runs.borrow_mut() += 1; }, ());
    });
    let after_mount = *runs.borrow();
    set_n.set(1);
    assert_eq!(*runs.borrow(), after_mount + 1);
    scope.dispose();
}

set_n.set(2);
assert_eq!(*runs.borrow(), 2);
```

## Scheduling

| item | what it does |
|------|--------------|
| [`batch(f)`] | one flush at the end of `f`, however many writes it makes |
| [`flush()`] | run the eager queue, then the deferred queue |
| [`tick()`] | timers, then futures, then both queues — what a frame calls |
| [`untrack(f)`] | read without subscribing |

```rust
use rok_ui_hooks::*;

let (a, set_a) = use_state(0);
let (b, set_b) = use_state(0);
let runs = use_ref(0);
let _effect = use_effect(
    { let (a, b, runs) = (a.clone(), b.clone(), runs.clone());
      move || { let _ = (a.get(), b.get()); *runs.borrow_mut() += 1; } },
    (),
);

let after_mount = *runs.borrow();
batch(|| { set_a.set(1); set_b.set(1); });
assert_eq!(*runs.borrow(), after_mount + 1);
assert_eq!(untrack(|| a.get() + b.get()), 2);
```

## Hooks

| item | what it does |
|------|--------------|
| [`use_reducer(initial, reducer) -> (ReadSignal<S>, Dispatch<S, A>)`] | one pure function for every transition |
| [`use_previous(source) -> Memo<Option<T>>`] | the value before the last run, `None` on mount |
| [`use_debounced(&source, quiet) -> Debounced<T>`] | trailing edge of a burst |
| [`use_throttled(&source, window) -> Throttled<T>`] | leading edge plus one trailing update |
| [`use_ref(v)`] | `Rc<RefCell<T>>` |

```rust
use rok_ui_hooks::*;

let (query, set_query) = use_state(String::from("a"));
let previous = use_previous(&query);
let debounced = use_debounced(&query, std::time::Duration::from_millis(0));

assert_eq!(previous.get(), None, "nothing before the first run");
set_query.set("b".into());
assert_eq!(previous.get(), Some("a".to_string()));
assert_eq!(debounced.get(), "a", "the timer has not fired yet");
tick();
assert_eq!(debounced.get(), "b");
```

## Resources

| item | what it does |
|------|--------------|
| [`use_resource(fetcher) -> Resource<T>`] | start a fetch, expose its state |
| [`use_resource_with(fetcher, select)`] | project the fetched value |
| `Resource::state() -> ReadSignal<ResourceState<T>>` | the tracked state signal |
| `Resource::get()` / `peek()` | state, tracked / untracked |
| `Resource::refetch()` | throw the value away and fetch again |
| [`ResourceState`] | `Idle`, `Loading`, `Ready(T)`, `Failed(String)` |
| `ResourceState::is_loading()` / `is_idle()` / `value()` / `error()` / `map(f)` | helpers |

```rust
use rok_ui_hooks::*;

let user = use_resource(|| async { 7 });
assert!(user.peek().is_loading());
tick();
assert_eq!(user.peek(), ResourceState::Ready(7));
user.refetch();
assert!(user.peek().is_loading(), "refetching resets to Loading");
tick();
assert_eq!(user.peek(), ResourceState::Ready(7));
```

## Futures

| item | what it does |
|------|--------------|
| [`spawn(future) -> Task`] | poll this future from `tick()` |
| `Task::cancel()` / `Task::is_finished()` | control the task |
| [`pending_tasks()`] | how many are still alive — for tests and diagnostics |

See [`async-and-time.md`](async-and-time.md).

## Stores

| item | what it does |
|------|--------------|
| [`create_store(initial) -> Store<T>`] | one mutable value with explicit subscribers |
| `Store::get()` / `with(f)` / `peek()` / `with_untracked(f)` | reads |
| `Store::set(v)` / `update(f)` | writes |
| `Store::select(f) -> Memo<U>` | a memo over a projection |
| `Store::select_with_eq(f, eq)` | the same with custom equality |
| `Store::subscribe(listener) -> Subscription<T>` | be told about every change, with old and new |
| [`use_store(&store, selector)`] | one call for "read a projection now" |

```rust
use rok_ui_hooks::*;

#[derive(Clone)]
struct User { name: String, email: String }
let store = create_store(User { name: "ada".into(), email: "ada@example.com".into() });

let name = store.select(|user: &User| user.name.clone());
assert_eq!(name.get(), "ada");

store.update(|user| {
    user.name = "grace".into();
    user.email = "grace@example.com".into();
});
assert_eq!(name.get(), "grace", "the selector re-ran");
assert_eq!(use_store(&store, |user| user.email.clone()), "grace@example.com");
```

## Context

| item | what it does |
|------|--------------|
| [`create_context(default) -> Context<T>`] | a typed, scoped value |
| [`with_provider(&ctx, value, f)`] | run `f` with `value` provided, and restore after |
| `Context::provide(value, f)` | the same, as a method |
| [`use_context(&ctx) -> T`] | the nearest provided value, else the default |
| `Context::get()` / `set(value)` | read or replace outside a provider |

```rust
use rok_ui_hooks::*;

let theme = create_context(String::from("light"));
let (seen, set_seen) = use_state(String::new());

// An effect created *inside* the provider sees the provided value.
with_provider(&theme, "dark".to_string(), || {
    let (theme, set_seen) = (theme.clone(), set_seen.clone());
    let _inner = use_effect(move || set_seen.set(theme.get()), ());
});
assert_eq!(seen.get(), "dark");

// The provider scoped its value: outside, the default is back.
assert_eq!(use_context(&theme), "light");
with_provider(&theme, "solarized".to_string(), || {
    assert_eq!(use_context(&theme), "solarized");
});
assert_eq!(use_context(&theme), "light");
```

## Keyed lists

| item | what it does |
|------|--------------|
| [`create_keyed_list(items, key, render) -> KeyedList<T, K, N>`] | one owned scope per key |
| `KeyedList::reconcile(items)` | diff by key: reuse, reorder, create, dispose |
| `KeyedList::entries()` | the outputs, in list order |
| `KeyedList::get(&key)` / `keys()` / `len()` / `is_empty()` | inspection |

```rust
use rok_ui_hooks::*;

let list = create_keyed_list(
    vec![(1, "one"), (2, "two")],
    |item: &(i32, &str)| item.0,
    |item: &(i32, &str)| format!("{}: {}", item.0, item.1),
);
assert_eq!(list.entries(), ["1: one", "2: two"]);

list.reconcile(vec![(2, "two"), (1, "ONE")]);
assert_eq!(list.entries(), ["2: two", "1: ONE"], "reordered, not rebuilt");
assert_eq!(list.keys(), [2, 1]);
```

## Dependency lists

`deps` is anything that implements [`Deps`]: a tuple of handles and/or values,
empty for "everything I read".

```rust
use rok_ui_hooks::*;

let (a, set_a) = use_state(0);
let (b, set_b) = use_state(0);
let runs = use_ref(0);
let _effect = use_effect(
    { let (a, runs) = (a.clone(), runs.clone()); move || { let _ = a.get(); *runs.borrow_mut() += 1; } },
    (a,),
);

let after_mount = *runs.borrow();
set_b.set(1);
assert_eq!(*runs.borrow(), after_mount, "deps win over what was read");
set_a.set(1);
assert_eq!(*runs.borrow(), after_mount + 1);
```