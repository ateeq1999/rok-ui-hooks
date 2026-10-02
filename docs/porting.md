# Porting

How to move code written against the pre-0.2 API of this crate, and how to move
code written against React or Solid.

## From React

The vocabulary maps one to one. The differences are all about ownership.

| React | rok-ui-hooks | note |
|-------|---------|------|
| `useState(0)` | `use_state(0)` | returns a **pair**, not a setter closure |
| `setX(v)` | `set_x.set(v)` | the write half is a value, not a function |
| `x` in render | `x.get()` | reads are explicit |
| `useEffect(fn, [a, b])` | `use_effect(f, (a, b))` | same shape, same subtlety |
| `useMemo(fn, [a])` | `use_memo(f, (a,))` | |
| `useRef(v)` | `use_ref(v)` | but it is `Rc<RefCell<T>>`, not reactive |
| `useReducer(reducer, init)` | `use_reducer(init, reducer)` | argument order flipped |
| `useContext(Ctx)` | `use_context(&ctx)` | |
| `<Ctx.Provider>` | `with_provider(&ctx, v, f)` | a scope, not markup |
| `useCallback` / `useMemo` for identity | not needed | there are no component props |
| cleanup returned from `useEffect` | `on_cleanup(f)` | explicit |

Two mechanical rules cover most ports:

1. **A signal handle is not `Copy`.** Clone it into a closure:

   ```rust
   use rok_ui_hooks::*;

   let (count, set_count) = use_state(0);
   // React: useEffect(() => console.log(count), [count]);
   let _effect = use_effect(
       { let count = count.clone(); move || println!("count = {}", count.get()) },
       (),
   );
   set_count.set(1);
   ```

2. **`'static` means clone the data you need.** A closure cannot borrow a local,
   so anything it reads is cloned in first — usually a cheap `Rc` handle, but
   sometimes the value.

Nothing else changes. There is no component tree, so there is no render phase and
nothing to reconcile.

## From Solid

Solid is a closer fit, because it is also fine-grained.

| Solid | rok-ui-hooks | note |
|-------|---------|------|
| `createSignal(v)` | `use_state(v)` | |
| `untrack(fn)` | `untrack(fn)` | identical |
| `createEffect(fn)` | `create_effect(fn, ())` | an explicit deps argument |
| `createRenderEffect(fn)` | `create_render_effect(fn, ())` | |
| `createMemo(fn)` | `use_memo(fn, ())` | |
| `createRoot(d => …)` | `create_root(\|\| …)` | same scoped lifetime |
| `onCleanup(fn)` | `on_cleanup(fn)` | |
| `createResource(fetcher)` | `use_resource(fetcher)` | polled by `tick()`, not by a scheduler |
| `createStore(v)` | `create_store(v)` | `store.update`, not a proxy |
| `unwrap(store.x)` | `store.select(\|s\| s.x.clone())` | explicit projections |
| `<For each={items}>` | `create_keyed_list(items, key, render)` | one owned scope per key |
| `batch(fn)` | `batch(fn)` | |
| Solid's scheduler | `tick()` | nothing runs without it |

The one real behavioural difference: Solid's runtime owns the executor and wakes
the graph itself. Here, you drive it. Call `tick()` once per frame, and a timer or
a future lands on the next frame at or after its deadline.

## From the pre-0.2 API of this crate

Version 0.2 replaced the old observer-stack design with a versioned graph and
added explicit ownership. Most code needs two changes.

### Effects are owned by a scope

Before, an effect lived until you dropped its handle. Now it lives until its
scope is disposed. Code that relied on "drop the handle to stop it" should own it
in a scope instead:

```rust
use rok_ui_hooks::*;

// Before: dropping the handle stopped the effect.
// Now: the scope owns it.
let (n, set_n) = use_state(0);
let runs = use_ref(0);

let scope = Root::new();
scope.run(|| {
    let (n, runs) = (n.clone(), runs.clone());
    let _effect = use_effect(move || { let _ = n.get(); *runs.borrow_mut() += 1; }, ());
});

set_n.set(1);
assert_eq!(*runs.borrow(), 2);

// …and to stop it early:
scope.dispose();
set_n.set(2);
assert_eq!(*runs.borrow(), 2, "the scope took the effect with it");
```

If you genuinely want "drop the handle to stop it", keep the handle and call
`effect.dispose()`.

### `Computation` is gone

The old public `Computation`, `Observer` and `tracked()` surface is replaced by:

| old | new |
|-----|-----|
| `tracked(f)` | just call `f` inside a computation |
| `untracked(f)` | `untrack(f)` |
| `Computation::get_state()` | nothing — it is internal now |

### `deps` is required

`use_effect(f)` became `use_effect(f, deps)`. The intent of each old call site:

- re-run on everything it read → `()`
- re-run when a value changes → `(value,)`
- run once → `(0i32,)` or `()`, plus a `use_ref` guard

### Memos take an equality argument

`use_memo(f, deps)` still bails out on `PartialEq`. If your value was a freshly
allocated collection, it now never bails out — pass `shallow_vec_eq` or a
comparator:

```rust
use rok_ui_hooks::*;

let store = create_store(vec![1, 2, 3]);
let rows = use_memo_eq(
    { let store = store.clone(); move || store.get() },
    (),
    shallow_vec_eq,
);
assert_eq!(rows.get(), vec![1, 2, 3]);
```

### Timers need a `tick()`

Debounce and throttle used to have a background thread. They now hold a deadline
and fire on the next `tick()`. In tests, `sleep` then `tick()`.

## Checklist

- [ ] Every closure that reads a signal clones the handle in.
- [ ] Every effect is created inside a scope that makes sense.
- [ ] `deps` arguments mean what you meant.
- [ ] Memos over collections have a comparator.
- [ ] The frame loop calls `tick()`.
- [ ] Tests that involve time call `tick()` after sleeping.