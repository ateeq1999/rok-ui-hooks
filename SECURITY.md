# Security policy

## Supported versions

| version | supported |
|---------|-----------|
| 0.2.x | yes |
| < 0.2 | no |

## Reporting a vulnerability

Please report security issues privately rather than in a public issue: use the
repository host's private vulnerability reporting if it is enabled, or contact the
maintainer directly through the address on the crate's `crates.io` page.

Include:

- what an attacker gains,
- the smallest program that shows it,
- the version or commit you tested.

You will get an acknowledgement within a few days, and a fix or a mitigation plan
once the report is confirmed.

## What is in scope

This crate is a reactive runtime with no I/O, no network, no filesystem access,
no `unsafe`, and no dependencies. The realistic security surface is therefore
small, but it is not empty:

- **Panics and denial of service in a host application.** If a graph can be made
  to panic — a `RefCell` double borrow, the effect-loop guard, a re-entrant
  `flush` — that is a denial of service in the program embedding it. Please
  report it.
- **Unbounded work.** A graph that grows without bound, or a future that is
  polled without limit, exhausts memory or CPU in the host.
- **Unsoundness.** There is none possible through `unsafe`, since there is none,
  but a `Rc`/`RefCell` misuse that produces a use-after-free in the graph's own
  structures would still count.

## What is not a vulnerability

- A panic from misuse you can see in the source, such as holding a
  `use_ref` borrow across a write. Documented in `docs/patterns.md`.
- The effect-loop guard panicking on an effect that writes what it reads. That is
  the designed behaviour.
- Untrusted input reaching your own formatting or allocation code.
- Anything requiring `unsafe` or a dependency you added yourself.