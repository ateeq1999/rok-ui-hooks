//! Signals, effects, memos and the deps gate.
//!
//! Convention used throughout: a handle needed by *both* the effect body and the
//! deps tuple is cloned for the body and left in place for the deps tuple.

mod common;

use std::cell::Cell;
use std::rc::Rc;

use common::log;
use signals::*;

// ───────────────────────────── state ─────────────────────────────

#[test]
fn state_get_set() {
    let (count, set_count) = use_state(3);
    assert_eq!(count.get(), 3);
    set_count.set(5);
    assert_eq!(count.get(), 5);
    set_count.update(|c| *c *= 2);
    assert_eq!(count.get(), 10);
}

// ───────────────────────────── effects ─────────────────────────────

#[test]
fn effect_reruns_on_change() {
    let (logs, push) = log();
    let (count, set_count) = use_state(0);
    let _e = use_effect(move || push(format!("The count is {}", count.get())), ());
    set_count.set(5);
    set_count.set(10);
    assert_eq!(
        *logs.borrow(),
        ["The count is 0", "The count is 5", "The count is 10"]
    );
}

#[test]
fn effect_reruns_when_a_listed_dep_changes() {
    let (logs, push) = log();
    let (count, set_count) = use_state(0);
    let _e = use_effect(
        {
            let (c, p) = (count.clone(), push.clone());
            move || p(format!("{}", c.get()))
        },
        (count,),
    );
    set_count.set(1);
    set_count.set(2);
    assert_eq!(*logs.borrow(), ["0", "1", "2"]);
}

#[test]
fn effect_skips_notification_from_unlisted_dep() {
    let (logs, push) = log();
    let (count, set_count) = use_state(1);
    let (other, set_other) = use_state("x");
    // `other` is read by the body (so it notifies) but is not a declared dep.
    let _e = use_effect(
        {
            let (c, o, p) = (count.clone(), other.clone(), push.clone());
            move || p(format!("{}-{}", c.get(), o.get()))
        },
        (count,),
    );
    set_other.set("y"); // gate closed: `count` unchanged
    assert_eq!(*logs.borrow(), ["1-x"]);
    set_count.set(2); // gate opens, and `other` has moved on
    assert_eq!(*logs.borrow(), ["1-x", "2-y"]);
}

#[test]
fn constant_deps_run_on_mount_only() {
    let (logs, push) = log();
    let (n, set_n) = use_state(0);
    let _e = use_effect(
        {
            let (n, p) = (n.clone(), push.clone());
            move || p(format!("mount {}", n.get()))
        },
        (0i32,),
    );
    set_n.set(1); // the body is re-notified, but the constant dep never changes
    set_n.set(2);
    assert_eq!(*logs.borrow(), ["mount 0"]);
}

#[test]
fn empty_deps_is_automatic_tracking() {
    let (logs, push) = log();
    let (a, set_a) = use_state(1);
    let (b, set_b) = use_state(2);
    let _e = use_effect(
        {
            let (a, b, p) = (a.clone(), b.clone(), push.clone());
            move || p(format!("{}{}", a.get(), b.get()))
        },
        (),
    );
    set_a.set(9);
    set_b.set(8);
    assert_eq!(*logs.borrow(), ["12", "92", "98"]);
}

// ───────────────────────────── cleanup ─────────────────────────────

#[test]
fn returned_cleanup_runs_before_rerun_and_on_dispose() {
    let (logs, push) = log();
    let (n, set_n) = use_state(0);
    let e = use_effect_with(
        move || {
            let v = n.get();
            push(format!("run {v}"));
            let p = push.clone();
            move || p(format!("cleanup {v}"))
        },
        (),
    );
    set_n.set(1);
    drop(e);
    set_n.set(2); // disposed → nothing
    assert_eq!(*logs.borrow(), ["run 0", "cleanup 0", "run 1", "cleanup 1"]);
}

#[test]
fn use_cleanup_runs_before_rerun_and_on_dispose() {
    let (logs, push) = log();
    let (n, set_n) = use_state(0);
    let p = push.clone();
    let e = use_effect(
        move || {
            let v = n.get();
            p(format!("run {v}"));
            let p2 = p.clone();
            use_cleanup(move || p2(format!("cleanup {v}")));
        },
        (),
    );
    set_n.set(1);
    drop(e);
    set_n.set(2); // disposed → nothing
    assert_eq!(*logs.borrow(), ["run 0", "cleanup 0", "run 1", "cleanup 1"]);
}

// ───────────────────────────── memos ─────────────────────────────

#[test]
fn memo_computes_once_for_many_readers() {
    let (logs, push) = log();
    let (first, set_first) = use_state("John".to_string());
    let (last, _set_last) = use_state("Smith".to_string());
    let p = push.clone();
    let full = use_memo(
        move || {
            p("compute".into());
            format!("{} {}", first.get(), last.get())
        },
        (),
    );
    let (f1, p1) = (full.clone(), push.clone());
    let _a = use_effect(
        {
            let (f, p) = (f1.clone(), p1);
            move || p(format!("My name is {}", f.get()))
        },
        (f1,),
    );
    let (f2, p2) = (full.clone(), push.clone());
    let _b = use_effect(
        {
            let (f, p) = (f2.clone(), p2);
            move || p(format!("Your name is not {}", f.get()))
        },
        (f2,),
    );
    set_first.set("Jacob".into());
    assert_eq!(
        *logs.borrow(),
        [
            "compute",
            "My name is John Smith",
            "Your name is not John Smith",
            "compute",
            "My name is Jacob Smith",
            "Your name is not Jacob Smith",
        ]
    );
}

#[test]
fn memo_skips_equal_values() {
    let (logs, push) = log();
    let (n, set_n) = use_state(1);
    let is_even = use_memo(
        {
            let n = n.clone();
            move || n.get() % 2 == 0
        },
        (n,),
    );
    let _e = use_effect(
        {
            let (m, p) = (is_even.clone(), push.clone());
            move || p(format!("even={}", m.get()))
        },
        (),
    );
    set_n.set(3); // still odd → memo value unchanged → effect not rerun
    set_n.set(4);
    assert_eq!(*logs.borrow(), ["even=false", "even=true"]);
}

#[test]
fn memo_works_with_derived_deps() {
    let (logs, push) = log();
    let (a, set_a) = use_state(1);
    let (b, set_b) = use_state(10);
    let total = use_memo(
        {
            let (a, b) = (a.clone(), b.clone());
            move || a.get() + b.get()
        },
        (a, b),
    );
    let _e = use_effect(
        {
            let (t, p) = (total.clone(), push.clone());
            move || p(format!("total {}", t.get()))
        },
        (total,),
    );
    set_b.set(20);
    set_a.set(2);
    assert_eq!(*logs.borrow(), ["total 11", "total 21", "total 22"]);
}

// ───────────────────────────── dynamic dependencies ─────────────────────────────

#[test]
fn dynamic_dependencies() {
    let (logs, push) = log();
    let (first, _) = use_state("John".to_string());
    let (last, set_last) = use_state("Smith".to_string());
    let (show_full, set_show_full) = use_state(true);
    let display = use_memo(
        {
            let (first, show_full, last) = (first.clone(), show_full.clone(), last.clone());
            move || {
                if !show_full.get() {
                    return first.get();
                }
                format!("{} {}", first.get(), last.get())
            }
        },
        (),
    );
    let _e = use_effect(
        {
            let (d, p) = (display.clone(), push.clone());
            move || p(format!("My name is {}", d.get()))
        },
        (display,),
    );

    set_show_full.set(false);
    set_last.set("Legend".into()); // nobody listens to `last` now → no log
    set_show_full.set(true);
    assert_eq!(
        *logs.borrow(),
        [
            "My name is John Smith",
            "My name is John",
            "My name is John Legend"
        ]
    );
}

// ───────────────────────────── untrack & batch ─────────────────────────────

#[test]
fn untrack_does_not_subscribe() {
    let (logs, push) = log();
    let (a, set_a) = use_state(1);
    let (b, set_b) = use_state(10);
    let _e = use_effect(
        {
            let (a, b, p) = (a.clone(), b.clone(), push.clone());
            move || {
                let b_val = untrack(|| b.get());
                p(format!("{}", a.get() + b_val));
            }
        },
        (a,),
    );
    set_b.set(20); // untracked → no rerun
    set_a.set(2);
    assert_eq!(*logs.borrow(), ["11", "22"]);
}

#[test]
fn batch_runs_effect_once() {
    let (logs, push) = log();
    let (a, set_a) = use_state(1);
    let (b, set_b) = use_state(2);
    let c = use_memo(
        {
            let b = b.clone();
            move || b.get() * 2
        },
        (b,),
    );
    let _e = use_effect(
        {
            let (a, c, p) = (a.clone(), c.clone(), push.clone());
            move || p(format!("sum {}", a.get() + c.get()))
        },
        (),
    );
    batch(|| {
        set_a.set(2);
        set_b.set(3);
    });
    assert_eq!(*logs.borrow(), ["sum 5", "sum 8"]);
}

#[test]
fn diamond_is_glitch_free() {
    let (logs, push) = log();
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
    let _e = use_effect(
        {
            let (double, triple, p) = (double.clone(), triple.clone(), push.clone());
            move || p(format!("{} {}", double.get(), triple.get()))
        },
        (),
    );
    set_a.set(2);
    // Without the batch in `notify` we'd also see the inconsistent "4 3".
    assert_eq!(*logs.borrow(), ["2 3", "4 6"]);
}

// ───────────────────────────── safety nets ─────────────────────────────

#[test]
fn self_write_does_not_loop() {
    let (n, set_n) = use_state(0);
    let runs = Rc::new(Cell::new(0));
    let r = runs.clone();
    let n2 = n.clone();
    let _e = use_effect(
        move || {
            r.set(r.get() + 1);
            let v = n2.get();
            if v < 100 {
                set_n.set(v + 1); // would recurse forever without the guard
            }
        },
        (),
    );
    assert_eq!(runs.get(), 1);
    assert_eq!(n.get_untracked(), 1);
}

#[test]
fn nested_effects_track_correctly() {
    let (logs, push) = log();
    let (name, set_name) = use_state("a");
    let (value, _) = use_state(1);
    let p = push.clone();
    let _outer = use_effect(
        {
            let (name, value, p) = (name.clone(), value.clone(), p.clone());
            move || {
                let (p2, value) = (p.clone(), value.clone());
                // The inner effect is owned by the outer run: dispose it on re-run.
                let inner = use_effect(move || p2(format!("value {}", value.get())), ());
                use_cleanup(move || drop(inner));
                p(format!("name {}", name.get())); // read AFTER the inner effect
            }
        },
        (),
    );
    set_name.set("b");
    assert_eq!(*logs.borrow(), ["value 1", "name a", "value 1", "name b"]);
}

#[test]
fn use_ref_survives_reruns() {
    let runs = Rc::new(Cell::new(0));
    let (n, set_n) = use_state(0);
    let r = runs.clone();
    let _e = use_effect(
        {
            let (n, r) = (n.clone(), r.clone());
            move || {
                n.get();
                r.set(r.get() + 1);
            }
        },
        (n.clone(),),
    );
    set_n.set(1);
    set_n.set(2);
    assert_eq!(runs.get(), 3);
    assert_eq!(n.get_untracked(), 2);
}
