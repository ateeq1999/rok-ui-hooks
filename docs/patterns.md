# Patterns and pitfalls

## Put state in one place

The cheapest graph is the one with the fewest nodes. Two rules of thumb:

- **One signal per fact.** Two signals that must change together are one signal
  (or a struct in one signal) plus a `batch`.
- **Derive, do not duplicate.** If a value can be computed from other values, it
  is a `use_memo`, not a second `use_state` kept in sync by hand.

Two sources of truth drift. This is how it starts:

```text
let (price, set_price) = use_state(100);
let (with_tax, set_with_tax) = use_state(120);
set_with_tax.set(price.get() * 120 / 100); // …and now remember to do this
                                          // every time anything changes
```

One fact, one derivation:

```rust
use signals::*;

let (price, set_price) = use_state(100);
let with_tax = use_memo(
    { let price = price.clone(); move || price.get() * 120 / 100 },
    (),
);
set_price.set(200);
assert_eq!(with_tax.get(), 240);
```

## Reducers for anything with rules

When transitions have rules ("increment", "toggle", "submit"), one pure function
beats a `match` scattered across event handlers — and it is trivially testable.

```rust
use signals::*;

#[derive(Clone, Debug, PartialEq)]
enum Action {
    Inc,
    Dec,
    Reset(i32),
}

let (count, dispatch) = use_reducer(0, |count: &i32, action: Action| match action {
    Action::Inc => count + 1,
    Action::Dec => count - 1,
    Action::Reset(value) => value,
});

dispatch.dispatch(Action::Inc);
dispatch.dispatch(Action::Inc);
dispatch.dispatch(Action::Dec);
dispatch.dispatch(Action::Reset(10));
assert_eq!(count.get(), 10);

// A dispatch is a handle: hand it to a child without re-borrowing state.
let stored = dispatch.clone();
stored.dispatch(Action::Dec);
assert_eq!(count.get(), 9);
```

## `use_previous` for diffing

```rust
use signals::*;

let (query, set_query) = use_state(String::new());
let previous = use_previous(&query);

let changes = use_ref(0);
let _effect = use_effect(
    {
        let (query, previous, changes) = (query.clone(), previous.clone(), changes.clone());
        move || {
            let current = query.get();
            if previous.get().as_deref() != Some(current.as_str()) {
                *changes.borrow_mut() += 1;
            }
        }
    },
    (),
);

let after_mount = *changes.borrow();
set_query.set("a".into()); // changed
set_query.set("a".into()); // same value: no re-run at all
assert_eq!(*changes.borrow(), after_mount + 1);
```

Note the direction: `use_previous` is a memo, so it is lazy and untracked until
read. Reading it inside the same effect that writes the signal gives you the
value from the previous run.

## Keyed lists

A keyed list keeps per-key state across reorders. It is the difference between a
list that works and a list that resets its first row's scroll position on every
insert.

```rust
use signals::*;

let list = create_keyed_list(
    vec![1, 2, 3],
    |id: &i32| *id,
    |id: &i32| {
        // Row state that must survive a reorder.
        let (visits, set_visits) = use_state(0);
        let _bump = use_effect(
            { let (visits, set_visits) = (visits.clone(), set_visits.clone());
              move || { let _ = visits.get(); set_visits.update(|n| *n += 1); } },
            (),
        );
        (format!("row {id}"), visits)
    },
);

let before = list.get(&2).expect("row 2 exists");
list.reconcile(vec![3, 2, 1]); // pure reorder
let after = list.get(&2).expect("row 2 still exists");
assert_eq!(after.0, before.0);
assert_eq!(after.1.get(), before.1.get(), "row state survived the move");

// A changed item re-renders that row; a new key creates one; a missing key is
// disposed, running its cleanups.
list.reconcile(vec![3, 2]);
assert_eq!(list.len(), 2);
```

Rules of thumb:

- The key must identify the thing, not its position.
- The render callback receives `&T`. A nested computation needs `'static`, so
  copy the fields it needs out of the reference first.
- Rows are disposed on `reconcile` and when the list is dropped. Nothing leaks.

## Context for what crosses a boundary

```rust
use signals::*;

let user = create_context(String::from("anonymous"));
let (shown, set_shown) = use_state(String::new());

with_provider(&user, "ada".to_string(), || {
    let (user, set_shown) = (user.clone(), set_shown.clone());
    let _child = use_effect(move || set_shown.set(use_context(&user)), ());
});

assert_eq!(shown.get(), "ada");
```

Use context for values that *many* things need (theme, locale, session) and not
for ordinary state you pass down — a context lookup is dynamic and invisible.

## Stores when you want subscribers, not a graph

A `Store` is a plain value with explicit listeners. Reach for it at the boundary
of a module that should not care *who* is watching.

```rust
use signals::*;

let store = create_store(vec![1, 2, 3]);
let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));

{
    let subscription = store.subscribe({
        let seen = seen.clone();
        move |new: &Vec<i32>, old: &Vec<i32>| seen.borrow_mut().push((old.len(), new.len()))
    });

    store.update(|items| items.push(4));
    drop(subscription); // dropping a Subscription unsubscribes
}
store.update(|items| items.push(5));

assert_eq!(*seen.borrow(), [(3, 4)], "the unsubscribed listener stopped");
```

## Pitfalls

### Reading a signal without a handle

The most common surprise: a closure that captured a signal by reference does not
compile (`'static`), and a `ReadSignal` is not `Copy`.

```rust
use signals::*;

let (n, set_n) = use_state(0);
let sink = use_ref(0);

// Wrong: `move || … n.get() …` would move `n` into the closure and it cannot be
// used afterwards. Clone the handle in.
let _effect = use_effect(
    { let (n, sink) = (n.clone(), sink.clone()); move || *sink.borrow_mut() = n.get() },
    (),
);
set_n.set(1);
assert_eq!(*sink.borrow(), 1);
```

### Writing what you read

An effect that writes a signal it also reads will loop. The runtime refuses to
hang and panics with a message instead — but the fix is structural: derive the
value, or write to a signal the effect never reads.

```rust
use signals::*;

// Fine: the effect reads `a` and writes `b`, which it never reads.
let (a, set_a) = use_state(0);
let (b, set_b) = use_state(0);
let _effect = use_effect(
    { let (a, set_b) = (a.clone(), set_b.clone()); move || set_b.set(a.get() + 1) },
    (),
);

set_a.set(5);
assert_eq!(b.get(), 6);
```

This is the loop that panics, and why — the fix is a memo, an untracked read, or
writing somewhere else:

```text
let (a, set_a) = use_state(0);
let _effect = use_effect(
    { let a = a.clone(); move || { let _ = a.get(); set_a.set(a.get() + 1); } },
    (),
);
set_a.set(1);
// → effect loop detected: an effect keeps writing to what it reads
//   (one node ran 100 times in a single flush)
```

### Memos with a fresh allocation

`PartialEq` on a freshly built `Vec` always differs, so every write re-runs every
downstream effect. Use `shallow_vec_eq` / `shallow_array_eq`, or
`use_memo_eq` with a comparator that compares what matters.

### Forgetting `deps`

`()` means "re-run when anything I read changes", which is usually what you want
and occasionally a performance bug. `(signal,)` is React's `[signal]`: re-run
only when its **value** changed.

### Effects outside a scope

A `use_effect` created at the top of a function has no owner if you never give it
one. Wrap it in `create_root`/`Root::run` when the work should die with that
function — otherwise it lives as long as the graph does.

### Assuming a timer fires on its own

It does not. Nothing in this crate runs without a `tick()`. In a test, that means
`sleep` then `tick()`; in an app, `tick()` once per frame.

### `Rc<RefCell<_>>` borrow across a call that may re-enter

`use_ref` returns a `RefCell`, and this crate is full of calls that can re-enter
an effect. Do not hold a borrow across a signal write:

```rust
use signals::*;

let items = use_ref(Vec::<i32>::new());
let (version, set_version) = use_state(0);
let (version, set_version) = (version.clone(), set_version.clone());
let _effect = use_effect(move || { let _ = version.get(); }, ());

// Wrong: items.borrow_mut() held while set_version runs an effect that touches
// items again → panic. Copy the data out, write, then drop the borrow.
let copy = items.borrow().clone();
items.borrow_mut().extend(copy);
set_version.update(|v| *v += 1);
assert_eq!(items.borrow().len(), 0);
```

## Testing reactive code

The graph is thread-local, so each test gets its own — no shared state, no
`#[serial]`, run tests in parallel for free.

```rust
use signals::*;

#[test]
fn a_write_settles_in_one_run() {
    let (a, set_a) = use_state(0);
    let runs = use_ref(0);
    let _effect = use_effect(
        { let (a, runs) = (a.clone(), runs.clone()); move || { let _ = a.get(); *runs.borrow_mut() += 1; } },
        (),
    );

    let after_mount = *runs.borrow();
    batch(|| { set_a.set(1); set_a.set(2); });
    assert_eq!(*runs.borrow(), after_mount + 1);
}
```

Useful tools while testing:

- `assert_eq!(signal.peek(), value)` — no subscription, no graph churn.
- `use_ref(Cell::new(0))` — count runs, or collect them into a `Vec`.
- `tick()` — advance timers and futures deterministically.
- `Root::new()` + `dispose()` — assert that a scope really released its work.