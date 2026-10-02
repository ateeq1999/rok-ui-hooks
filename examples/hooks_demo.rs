//! Every hook, side by side with the React API it mirrors.
//!
//! ```text
//! cargo run --example hooks_demo
//! ```

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use signals::*;

fn main() {
    state_like_use_state();
    println!();
    effects_and_deps();
    println!();
    memos_are_lazy_and_cached();
    println!();
    refs_and_cleanup();
    println!();
    untrack_and_batch();
}

/// `useState` — a readable/writable pair, and the only way to hold state.
fn state_like_use_state() {
    println!("— use_state —");
    let (count, set_count) = use_state(0);
    set_count.update(|c| *c += 5);
    println!("count = {}", count.get()); // 5 — no closures, no rerun
}

/// `useEffect(fn, deps)` — the workhorse. `deps` gates *when* it re-runs, while
/// the body itself declares *what* it depends on (fine-grained tracking).
fn effects_and_deps() {
    println!("— use_effect / deps —");
    let (count, set_count) = use_state(0);
    let (other, set_other) = use_state("x");

    // `()` → re-run when anything the body read changes (Solid-style).
    let auto = use_effect(
        {
            let (count, other) = (count.clone(), other.clone());
            move || println!("  ()        -> {}{}", count.get(), other.get())
        },
        (),
    );

    // `(count,)` → only `count` gates it, like React's `[count]`.
    let gated = use_effect(
        {
            let (count, other) = (count.clone(), other.clone());
            move || println!("  (count,)  -> {}{}", count.get(), other.get())
        },
        (count.clone(),),
    );

    // `(0i32,)` → a constant never changes, so this is React's `[]`: mount only.
    let _mount_only = use_effect(
        {
            let (count, other) = (count.clone(), other.clone());
            move || println!("  (0i32,)   -> mount, saw {}{}", count.get(), other.get())
        },
        (0i32,),
    );

    set_count.set(1); // both open gates
    set_other.set("y"); // only the `()` gate opens
    set_count.set(2);
    drop((auto, gated));
}

/// `useMemo(fn, deps)` — a cached derivation that other computations read
/// instead of recomputing, and that notifies only when its value changes.
fn memos_are_lazy_and_cached() {
    println!("— use_memo —");
    let (n, set_n) = use_state(1);
    let parities = Rc::new(RefCell::new(Vec::new()));

    let parity = use_memo(
        {
            let (n, parities) = (n.clone(), parities.clone());
            move || {
                parities.borrow_mut().push(n.get_untracked());
                if n.get() % 2 == 0 { "even" } else { "odd" }
            }
        },
        (n.clone(),),
    );

    let report = use_effect(
        {
            let (n, parity) = (n.clone(), parity.clone());
            move || println!("  {} is {}", n.get(), parity.get())
        },
        (),
    );

    set_n.set(3); // recomputes (odd), value unchanged → the effect stays quiet
    set_n.set(4); // recomputes (even) → the effect runs
    println!("  memo recomputed for: {:?}", parities.borrow()); // only on change
    drop(report);
}

/// `useRef` / `useCleanup` / a cleanup returned from the effect body.
fn refs_and_cleanup() {
    println!("— use_ref / use_cleanup —");

    // A ref is plain owned state: it is not a source, so reading it never
    // re-runs the effect, and it survives across runs.
    let runs = Rc::new(Cell::new(0));
    let (n, set_n) = use_state(0);
    let effect = use_effect(
        {
            let (n, runs) = (n.clone(), runs.clone());
            move || {
                runs.set(runs.get() + 1); // run #1, #2, #3 …
                n.get();
                println!("  run {}, n = {}", runs.get(), n.get());
            }
        },
        (n.clone(),),
    );
    set_n.set(1);
    set_n.set(2);

    // A cleanup returned from the body, React-style.
    let connection = use_effect_with(
        {
            let runs = runs.clone();
            move || {
                let attempt = runs.get() + 1;
                println!("  connect  (attempt {attempt})");
                move || println!("  disconnect (after {attempt} run)")
            }
        },
        (),
    );

    // `use_cleanup` registers the same kind of cleanup from inside the body.
    let (label, set_label) = use_state("draft");
    let typed = use_effect(
        {
            let (label, runs) = (label.clone(), runs.clone());
            move || {
                runs.set(runs.get() + 1);
                println!("  editing {}", label.get());
                let label = label.get().to_string();
                use_cleanup(move || println!("  saved {label}"));
            }
        },
        (label,),
    );
    set_label.set("published");

    drop((effect, connection, typed));
    println!("  (effects disposed)");
}

/// `untrack` opts out of tracking; `batch` collapses many writes into one run.
fn untrack_and_batch() {
    println!("— untrack / batch —");
    let (a, set_a) = use_state(1);
    let (b, set_b) = use_state(10);

    let _effect = use_effect(
        {
            let (a, b) = (a.clone(), b.clone());
            move || {
                let b_now = untrack(|| b.get()); // not a dependency
                println!("  a + b = {}", a.get() + b_now);
            }
        },
        (a,),
    );

    set_b.set(20); // untracked → no run
    batch(|| {
        set_a.set(2);
        set_b.set(30);
    }); // two writes, one run
}
