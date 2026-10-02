# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project follows
[semantic versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.3.0] - 2026-10-02

### Added

- **Feature flags.** Five optional features, all on by default, so a plain
  `rok-ui-hooks = "0.3"` still gets the whole surface:

  | feature | brings |
  |---------|--------|
  | `store` | `create_store`, `Store`, `use_store` |
  | `context` | `create_context`, `use_context`, `with_provider` |
  | `keyed` | `create_keyed_list`, `KeyedList` |
  | `async` | `spawn`, `Task`, `use_resource` |
  | `timers` | `use_debounced`, `use_throttled` |

  `full` is an alias for all five and is what `default` points at.
  `default-features = false` plus an explicit list is supported, and CI checks
  all 32 subsets with `-D warnings`.
- `required-features` on every example, test and bench target, so
  `cargo test --no-default-features --features store` runs the tests it can and
  skips the rest instead of failing to compile.

### Changed

- `tick()` compiles down to `flush()` when `timers` and `async` are both off —
  there is then no deadline to fire and no task to poll. The symbol is unchanged,
  so a feature-gated loop needs no conditional around the call.
- The provider machinery in `context.rs` (the stack, `ProviderFrame`, capture and
  restore) stays compiled when `context` is off. `runtime.rs` re-installs a
  captured stack on every run of every node and that is not feature-dependent,
  so the feature removes the public API rather than the plumbing.

### Fixed

- `README.md` described `hooks_demo` as demonstrating debounce and throttle. It
  does not; the debounce/throttle examples live in `async` and are exercised by
  `tests/scheduler.rs`.

## [0.2.0] - 2026-10-02

### Added

- **Ownership.** `create_root`, `Root`, `create_owned_root`, `OwnedRoot`. Every
  hook is owned by the scope that created it, so disposal is exact: `tear_down`
  walks strong child edges depth-first and runs cleanups. Re-running an effect now
  disposes the nested computations of the previous generation instead of leaking
  them.
- **Scheduling.** `batch`, `flush`, `tick`, `untrack`, `create_deferred_effect`,
  `Effect::dispose`, `Effect::leak`, `Effect::is_alive`.
- **Hooks.** `use_reducer` / `Dispatch`, `use_previous`, `use_debounced` /
  `Debounced`, `use_throttled` / `Throttled`, `use_ref`, `use_cleanup`,
  `shallow_vec_eq`, `shallow_array_eq`, `use_memo_eq`.
- **Async.** `use_resource`, `use_resource_with`, `Resource`, `ResourceState`,
  `spawn`, `Task`, `pending_tasks`. Futures are polled from `tick()` on the
  calling thread, with latest-wins and cancellation-on-dispose semantics.
- **Keyed lists.** `create_keyed_list` / `KeyedList`: one owned scope per key,
  reuse across reorders, explicit disposal for removed and changed rows.
- **Docs.** `README.md` and a `docs/` tree whose code blocks are compiled as
  doctests, plus examples for the new APIs (`ownership`, `keyed_list`, `async`).
- `cargo bench`: a zero-dependency benchmark that checks the cost model.

### Changed

- **Graph rewrite.** Nodes carry a version and a `Clean` / `Check` / `Dirty`
  state. A memo that recomputes to an equal value no longer wakes its readers, so
  a diamond updates once instead of cascading.
- **Queues are ordered by node id** and drained with `pop_first`, which is what
  makes every update glitch-free: work runs in creation order, so a memo is
  refreshed before the effects that read it.
- **Timers are thread-local.** Debounce and throttle hold a deadline and fire on
  the next `tick()` at or after it. There is no timer thread, and user callbacks
  no longer need `Send`.
- **`spawn` inside a scope is owned by that scope** and cancelled when it is
  disposed; outside a scope, dropping the handle cancels, as before.
- A future that wakes itself is polled at most 100 times per `tick()` and then
  left for the next one, instead of panicking.
- An effect that keeps writing to what it read panics with a message naming the
  problem, instead of hanging.
- `Effect::dispose` on a disposed effect is idempotent, and disposal during
  teardown no longer trips a `RefCell` borrow.
- Store selectors are memos: they run at most once per change and only notify
  when the projected value differs.
- Context providers propagate: an effect under a provider re-runs when the
  provided value changes.
- `use_effect` takes an explicit `deps` argument everywhere.

### Removed

- `Computation`, `Observer` and the public `tracked()` entry point. Reading a
  signal inside a computation is what subscribes; use `untrack` to opt out.
- The background timer thread and its callback handoff.
- `use_shallow_memo`.

### Fixed

- A running observer subscribed the *source* instead of itself, so nested
  computations could read each other's dependencies.
- Memos recomputed against a stale cache when mounted inside an effect.
- The default memo comparator treated "changed" as "equal".
- Strong ownership cycles: a node's subscribers are `Weak`, only its owned
  children are `Rc`.
- Nested cleanup ran while a `RefCell` borrow was held, panicking on the second
  level.
- Stale keyed rows were dropped without disposal, skipping their cleanups.
- A store write inside a listener panicked instead of queueing.

