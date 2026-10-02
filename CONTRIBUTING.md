# Contributing

Thanks for looking. This crate is small on purpose, so the bar for a change is
mostly "does it keep it small".

## Ground rules

1. **No dependencies.** Runtime or dev. A reactive core that pulls a tree is a
   different project.
2. **No `unsafe`.** `#![forbid(unsafe_code)]` is not negotiable.
3. **No `Send` on the core path.** The graph is thread-local by design; see
   `ROADMAP.md` before proposing otherwise.
4. **Behaviour changes need a test that fails without them.** Reactive bugs hide
   in graphs nobody exercised.
5. **Documentation is part of the change.** Every code block in `README.md` and
   `docs/*.md` is compiled as a doctest, so an example that stops working breaks
   `cargo test`.

## Workflow

```text
cargo test          # unit, integration, and doctests (including the docs)
cargo clippy --all-targets --all-features
cargo fmt
cargo bench         # if you touched the cost model
cargo run --example <name>
```

Before opening a pull request:

- [ ] `cargo test` passes, including every `docs/*.md` example.
- [ ] `cargo clippy --all-targets` is warning-free.
- [ ] `cargo fmt --check` is clean.
- [ ] New public items have doc comments with an example.
- [ ] `CHANGELOG.md` has an entry under `Unreleased`.
- [ ] Anything user-visible is in `README.md` or `docs/`.

## Tests

The graph is thread-local, so every test gets its own and tests run in parallel
without coordination. Use that:

- Assert observable state (`signal.peek()`, a `use_ref` log) rather than internal
  counters.
- Prefer `batch` to "do the writes and hope"; it makes the test describe the
  ordering it depends on.
- Time is injected: `std::thread::sleep` then `tick()`. No fake clock, no
  `#[serial]`.
- Disposal is worth asserting: create a scope, write to a signal, dispose, write
  again, assert nothing ran.

## Commit messages

One line, imperative, under 72 characters. Explain *why* in the body when the
reason is not obvious from the diff — especially for anything that trades
performance for clarity, or clarity for performance.

## Reporting bugs

Include the smallest program that shows it. A reactive bug is usually a
subscription graph, so the answer is usually in the example: which effect reads
which signal, and in what order the writes happen. `cargo run --example demo`
is a good starting shape.