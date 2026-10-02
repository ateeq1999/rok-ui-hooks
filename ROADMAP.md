# Roadmap

Where this crate is going, and what it deliberately is not doing yet. Everything
here is a decision about *not* writing code until there is a reason.

## 0.2.x — hardening

- `#[deny(missing_docs)]` once the public surface stops moving.
- `Send + Sync` on the read-only halves (`ReadSignal`, `Memo`) so state can cross
  a thread boundary read-only.
- Property tests for the state machine: any interleaving of writes, batches and
  disposals converges to the same value as a sequential reference.
- A `loom` model of the subscription bookkeeping.

## 0.3 — a real scheduler

Today `tick()` is the only clock, which is right for a frame loop and wrong for a
terminal program. Planned:

- An optional background driver that wakes the thread owning the graph when a
  deadline passes or a future becomes ready.
- `tick(Duration)` and a `Scheduler` that owns the queue, so `rok-ui-hooks` can drive
  a blocking program without the caller writing a loop.
- Timer precision that does not depend on the caller polling.

Design constraint: the driver may *notify*, but the graph must still be mutated
on the thread that owns it. The `Rc`/`RefCell` core stays.

## 0.4 — multi-threaded engine (not before)

An `Arc`-backed graph, behind a feature flag, for programs that genuinely need
several threads to write to one graph. This is a large change:

- Every read becomes an atomic load or a lock, and the cost model in
  `docs/architecture.md` gets worse.
- Effects must declare whether they may run on another thread, or the engine must
  ship a thread-affinity scheduler.
- `Send` bounds appear in every user closure, which is a real ergonomic cost for
  the 99% case that is single-threaded.

It will not be done speculatively. If you need it, open an issue describing the
shape of your workload.

## Not planned

| idea | why not |
|------|---------|
| A JSX-like component layer | The crate is the runtime. A component layer is a separate crate, and a second one already exists. |
| `Copy` handles into an arena | Stable handles into a slab need a generation counter or a borrow, and both are more error-prone than an `Rc` clone that the optimiser elides. |
| Devtools / a graph visualiser | Valuable, and a separate tool. A `Debug` for every node is the prerequisite, and that exists. |
| An `async` runtime integration | `spawn` + `tick` is enough. Hooking `tokio` would add a dependency to a crate with none. |
| Fine-grained error handling in effects | `use_resource` covers the case that matters. A general answer needs `Result` in every signature. |

## Non-goals

- Replacing React or Solid. This is the reactive core, deliberately without a
  component model.
- Being the fastest thing available. The benchmark exists to keep the cost model
  honest, not to win a contest.
- Supporting stable-Rust features newer than the MSRV. The MSRV is tested in CI
  and will not be raised without a major version.