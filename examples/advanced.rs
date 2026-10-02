//! The engine's less obvious guarantees, each in one small demo: dynamic
//! dependencies, the deps gate, glitch-free diamonds, `untrack`, `batch`,
//! nesting, and the self-write guard.
//!
//! ```text
//! cargo run --example advanced
//! ```

use std::cell::Cell;
use std::rc::Rc;

use rok_ui_hooks::*;

fn main() {
    dynamic_dependencies();
    println!();
    the_deps_gate();
    println!();
    diamond_is_glitch_free();
    println!();
    untrack_and_dynamic_reports();
    println!();
    batch_is_transactional();
    println!();
    nested_effects_are_owned();
    println!();
    self_write_is_guarded();
}

/// A memo can subscribe to *different* signals on different runs; only the
/// dependencies of the latest run are kept.
fn dynamic_dependencies() {
    println!("— dynamic dependencies —");
    let (first, _) = use_state("John".to_string());
    let (last, set_last) = use_state("Smith".to_string());
    let (show_full, set_show_full) = use_state(true);

    let display = use_memo(
        {
            let (first, last, show_full) = (first.clone(), last.clone(), show_full.clone());
            move || {
                if !show_full.get() {
                    return first.get();
                }
                format!("{} {}", first.get(), last.get())
            }
        },
        (),
    );
    let log = use_effect(
        {
            let display = display.clone();
            move || println!("  My name is {}", display.get())
        },
        (),
    );

    set_show_full.set(false); // now depends on `first` only…
    set_last.set("Legend".into()); // …so this is now free
    set_show_full.set(true); // `last` is back in the graph
    drop(log);
}

/// The deps tuple gates re-entry; the body decides what is tracked.
fn the_deps_gate() {
    println!("— the deps gate —");
    let (count, set_count) = use_state(0);
    let (noise, set_noise) = use_state(0);

    let gated = use_effect(
        {
            let (count, noise) = (count.clone(), noise.clone());
            move || println!("  ran: count={} noise={}", count.get(), noise.get())
        },
        (count,),
    );

    set_noise.set(1); // notified, but `count` did not change → gated out
    set_noise.set(2);
    println!("  (two noise writes, no runs)");
    set_count.set(1); // gate opens once
    set_noise.set(3); // gate closed again
    println!("  (one count write, one run)");
    drop(gated);
}

/// A computation that reads two memos of the same signal must never observe an
/// inconsistent intermediate state, even without an explicit `batch`.
fn diamond_is_glitch_free() {
    println!("— glitch-free diamonds —");
    let (a, set_a) = use_state(1);
    let a2 = a.clone();
    let double = use_memo(
        {
            let a3 = a2.clone();
            move || a3.get() * 2
        },
        (a2,),
    );
    let triple = use_memo(
        {
            let a = a.clone();
            move || a.get() * 3
        },
        (a,),
    );
    let report = use_effect(
        {
            let (double, triple) = (double.clone(), triple.clone());
            move || println!("  double={} triple={}", double.get(), triple.get())
        },
        (),
    );
    set_a.set(2); // never "double=4 triple=3"
    drop(report);
}

/// `untrack` for reads that are diagnostics only, plus a value that changes
/// without changing the report.
fn untrack_and_dynamic_reports() {
    println!("— untrack —");
    let (items, set_items) = use_state(vec![1, 2, 3]);
    let (query, set_query) = use_state(String::new());

    let report = use_effect(
        {
            let (items, query) = (items.clone(), query.clone());
            move || {
                let q = query.get().to_lowercase();
                // `items` is only inspected to build the log line, so don't let
                // it subscribe the effect.
                let count = untrack(|| items.get().len());
                println!("  query={q:?} over {count} item(s)");
            }
        },
        (query,),
    );

    set_query.set("s".into());
    set_items.update(|v| v.push(4)); // no rerun: it was untracked
    set_items.update(Vec::clear); // no rerun either
    set_query.set("z".into());
    println!("  items is now {:?}", items.get_untracked());
    drop(report);
}

/// Writes inside a `batch` are invisible to observers until the end.
fn batch_is_transactional() {
    println!("— batch —");
    let (first, set_first) = use_state(0);
    let (last, set_last) = use_state("".to_string());
    let saved = use_effect(
        {
            let (first, last) = (first.clone(), last.clone());
            move || println!("  SAVE draft: {} {}", first.get(), last.get())
        },
        (),
    );

    set_first.set(1);
    set_last.set("one".into());
    println!("  two unbatched writes → two saves");

    batch(|| {
        set_first.set(2);
        set_last.set("two".into());
    });
    println!("  one batch → one save");
    drop(saved);
}

/// An effect created inside another effect is owned by it: it is disposed when
/// the outer effect re-runs, and the outer effect stays the observer.
fn nested_effects_are_owned() {
    println!("— nested effects —");
    let (parent, set_parent) = use_state("A");
    let (child, _) = use_state(1);

    let log = use_effect(
        {
            let (parent, child) = (parent.clone(), child.clone());
            move || {
                let inner = use_effect(
                    {
                        let child = child.clone();
                        move || println!("    child runs (child={})", child.get())
                    },
                    (),
                );
                use_cleanup(move || {
                    println!("    disposing inner effect");
                    drop(inner);
                });
                println!("  parent runs (parent={})", parent.get());
            }
        },
        (),
    );
    set_parent.set("B");
    println!("  child signal changed while parent stayed put → no run");
    drop(log);
}

/// Writing a signal from the effect that reads it must not recurse forever:
/// the write is ignored for the current run.
fn self_write_is_guarded() {
    println!("— self-write guard —");
    let (n, set_n) = use_state(0);
    let runs = Rc::new(Cell::new(0));
    let kept = use_effect(
        {
            let (n, runs) = (n.clone(), runs.clone());
            move || {
                runs.set(runs.get() + 1);
                let v = n.get();
                println!("  run {}: saw {v}", runs.get());
                if v < 5 {
                    set_n.set(v + 1); // deferred: only the *next* notification sees it
                }
            }
        },
        (),
    );
    println!(
        "  effect ran {} time(s), n is now {}",
        runs.get(),
        n.get_untracked()
    );
    drop(kept);
}
