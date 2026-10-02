# Architecture

How the crate is put together, for when you need to change it. Everything lives
in `thread_local!` state; there is no global lock and no `Send` anywhere.

## Files

| file | what lives there |
|------|------------------|
| `runtime.rs` | the graph: nodes, states, tracking, queues, batches, ownership |
| `signal.rs` | `ValueCell`, `ReadSignal`, `WriteSignal` |
| `memo.rs` | lazy cached derivations and comparators |
| `effect.rs` | effects, cleanups, refs, `deps` plumbing |
| `hooks.rs` | reducer, previous, debounce, throttle, `OwnedRoot` |
| `keyed.rs` | keyed lists, one owned scope per key |
| `resource.rs` | resources and their state machine |
| `executor.rs` | the local future executor |
| `timer.rs` | deadlines and their callbacks |
| `context.rs` | typed scoped values with propagation |
| `store.rs` | stores, selectors, subscriptions |
| `deps.rs` | what a `deps` argument may be |

## Feature flags

Five optional features gate the public API; all are on by default, so a plain
dependency gets everything and the core is unaffected either way.

| feature | gates | shape of the gate |
|---------|-------|-------------------|
| `store` | `store.rs` | whole module |
| `keyed` | `keyed.rs` | whole module |
| `async` | `executor.rs`, `resource.rs` | whole module |
| `context` | `context.rs` | public items only — see below |
| `timers` | `timer.rs`, and `Debounced`/`Throttled` in `hooks.rs` | whole module + four items |

Three couplings make this more than a mechanical `#[cfg]` sweep, and each one is
a trap for whoever changes the graph next:

- **`context` is not self-contained.** `runtime.rs` stores a captured provider
  stack on every `Node` and re-installs it in `with_running`, so the stack, the
  `ProviderFrame` and the capture/restore pair stay compiled even when the
  feature is off — only `Context`, `create_context`, `with_provider`,
  `use_context` and `resolve` disappear. Making this a feature-gated module
  would mean either stubbing the plumbing in `Node` or paying a branch on the
  hot path.
- **`Node::reset` rides `keyed`.** Its only caller is
  `KeyedList::render_entry`, which re-renders a row in the same scope.
- **`WriteSignal::node` rides `store`.** Its only caller is `Store::set`, which
  notifies its own listeners.

Both of those carry a comment saying so. A new caller gets a compile error at
the call site rather than a mysterious dead-code warning in the other direction,
which is why they are `#[cfg]` rather than `#[allow(dead_code)]`.

`tick()` is the one function whose *behaviour* changes rather than its body
shrinking: without `timers` and `async` there is no deadline to fire and no task
to poll, so it reduces to `flush()`. The symbol stays, because a caller with a
feature-gated loop should not need a conditional to keep calling it.

CI checks all 32 feature subsets with `-D warnings`, so a dead-code or
unused-import regression in any combination fails the build rather than waiting
for a user to hit it.

## Nodes

Everything computable is a `Node`:

```text
Node
├─ id            identity, and the key in the registry
├─ state         Clean | Check | Dirty
├─ version       bumped when an upstream value changes
├─ sources       Weak<Node> — what this node read last run
├─ subs          Weak<Node> — who read this node last run
├─ children      Rc<Node> — nested computations this node owns
├─ cleanups      Box<dyn FnOnce()>
├─ deps          optional `deps` gate
└─ mode          Eager | Deferred | Render
```

`Rc<Node>` with `Weak` edges is what makes the graph cheap *and* collectable: a
subscriber does not keep its source alive, so dropping the root really does
release the graph.

### The state machine

```text
write to S
   │
   ├─ S.version += 1
   └─ for each subscriber of S:
        mark_subscriber(node)

mark_subscriber(node)
   │
   ├─ memo, and version unchanged  → Check   (recompute only if read)
   ├─ memo, and version changed    → Dirty  (and propagate to its subs)
   └─ effect                       → queue for the flush
```

`Check` is the important one. It is what stops a diamond from cascading: two
derived values that come out equal never mark what reads them.

## Queues and ordering

Three ordered collections, all keyed by node id, so a node is queued at most
once per flush:

```text
PENDING    eager effects, drained when the outermost batch ends
DEFERRED   deferred effects, drained by flush() after PENDING
READY      futures, drained by tick() before the queues
```

The queue is a `BTreeMap<Id, _>` and `pop_first` is used, so work runs in
**creation order**. A memo created before an effect is therefore refreshed
before that effect runs: the effect sees one consistent graph, and no reader ever
observes an intermediate value. That is the whole glitch-free argument, and it is
one line of data-structure choice.

Writes made *by* an effect are queued, never run re-entrantly, so the graph never
re-enters itself. An effect that keeps writing to what it read trips the loop
guard (`MAX_EFFECT_RUNS`) and panics with an explanation instead of hanging.

## Ownership

```text
Root { node }                       an owner node with no parent
  └─ run(f) → f runs with the owner on the ownership stack

Node.children: RefCell<Vec<Rc<Node>>> strong
Node.sources / Node.subs:           Weak
```

Strong child edges are what make disposal exact: `tear_down` walks children
depth-first, runs cleanups, and drops each child. No global sweep, no "is it
still reachable" analysis, no leak.

A hook created while an effect is running belongs to that effect, which is why
re-running an effect disposes the previous generation instead of leaking it.

## Scheduling

```text
write ─► mark ─► queue ─► (outermost batch ends) ─► drain(PENDING)
                                                      └─ writes during drain
                                                         re-queue, next pass

tick() = timer::drain() ─► executor::run_ready() ─► flush() = drain(PENDING) ─► drain(DEFERRED)
```

`batch` only changes *when* the drain happens, never *what* it does. Nested
batches increment a depth counter; only depth 0 drains.

## Futures and timers

Both are thread-local and both are driven by `tick()`:

- The executor holds `TASKS: RefCell<BTreeMap<u64, Job>>` plus a `READY` deque.
  A waker is an `Arc<WakeById>` that re-enqueues an id. A task is polled at most
  `MAX_POLLS_PER_TICK` times per tick and is then left for the next one.
- Timers are a `BinaryHeap<Reverse<(Instant, u64)>>` plus a token → callback
  map. `drain` pops every deadline that has passed and runs the callback on the
  calling thread. Cancelling removes the callback; the heap entry is dropped when
  it comes up.

Neither requires `Send`, because neither ever leaves the thread.

## Costs

| operation | cost |
|-----------|------|
| tracked read | `RefCell` borrow + version compare + one `Rc` clone into `sources` |
| untracked read | `RefCell` borrow |
| write | version bump + iterate subscribers |
| memo read | compare versions; recompute only when needed |
| memo recompute | one run, plus propagate if the value changed |
| flush | proportional to queued work, in creation order |

`cargo bench` prints the actual numbers for this machine; it is a smoke test of
the cost model, not a competitive benchmark.

## Why `thread_local!`

- `Rc` + `RefCell` instead of `Arc` + `Mutex`: no atomics on the hot path.
- User closures need no `Send`, so they can capture anything the thread owns.
- A frame cannot be interleaved with a background write.

The cost is that a graph belongs to one thread. `ROADMAP.md` records what an
`Arc`-backed engine would need, and why this crate waits for a real use case.

## Testing

The graph is thread-local, so every test binary — and every test in it — gets its
own graph on its own thread. Tests are independent by construction, and
`cargo test` runs them in parallel with no serialisation.

Time is injected: tests sleep and call `tick()`, so debounce and throttle tests
are deterministic without a fake clock or a `tokio::time::pause`.