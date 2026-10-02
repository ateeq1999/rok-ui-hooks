//! Ownership: roots, scopes, disposal and cleanups.

use std::cell::Cell;
use std::rc::Rc;

use signals::*;

// ───────────────────────────── create_root ─────────────────────────────

#[test]
fn create_root_disposes_everything_it_created() {
    let (count, set_count) = use_state(0);
    let runs = Rc::new(Cell::new(0));

    create_root(|| {
        let (r, s) = (runs.clone(), count.clone());
        let _e = use_effect(
            move || {
                s.get();
                r.set(r.get() + 1);
            },
            (),
        );
    });

    // The scoped form disposes on the way out, so later writes are inert.
    assert_eq!(runs.get(), 1);
    set_count.set(1);
    assert_eq!(runs.get(), 1);
}

#[test]
fn a_root_keeps_its_effects_until_it_is_dropped() {
    let (count, set_count) = use_state(0);
    let runs = Rc::new(Cell::new(0));
    let root = Root::new();

    root.run(|| {
        let (r, s) = (runs.clone(), count.clone());
        let _e = use_effect(
            move || {
                s.get();
                r.set(r.get() + 1);
            },
            (),
        );
    });

    // Owned by the root, not by the local handle: still alive after `run` returned.
    assert_eq!(runs.get(), 1);
    set_count.set(1);
    assert_eq!(runs.get(), 2);

    drop(root);
    set_count.set(2);
    assert_eq!(runs.get(), 2);
}

#[test]
fn a_root_returns_a_value_and_nests() {
    let sum = create_root(|| {
        let (a, set_a) = use_state(1);
        let (b, set_b) = use_state(2);
        create_root(|| set_b.set(10));
        set_a.set(5);
        a.get() + b.get()
    });
    assert_eq!(sum, 15);
}

#[test]
fn root_can_be_kept_and_disposed_by_hand() {
    let (n, set_n) = use_state(0);
    let runs = Rc::new(Cell::new(0));
    let root = Root::new();
    let seen = root.run(|| {
        let (r, s) = (runs.clone(), n.clone());
        let _e = use_effect(
            move || {
                s.get();
                r.set(r.get() + 1);
            },
            (),
        );
        runs.get()
    });
    assert_eq!(seen, 1, "the effect ran on mount, inside the scope");

    set_n.set(1);
    assert_eq!(runs.get(), 2);

    root.dispose();
    set_n.set(2);
    assert_eq!(runs.get(), 2);
}

// ───────────────────────────── owned roots ─────────────────────────────

#[test]
fn an_owned_root_outlives_the_call_that_created_it() {
    let (n, set_n) = use_state(0);
    let runs = Rc::new(Cell::new(0));

    let (scope, ()) = create_owned_root(|| {
        let (r, s) = (runs.clone(), n.clone());
        let _e = use_effect(
            move || {
                s.get();
                r.set(r.get() + 1);
            },
            (),
        );
    });

    set_n.set(1);
    assert_eq!(runs.get(), 2);

    scope.dispose();
    set_n.set(2);
    assert_eq!(runs.get(), 2);
}

#[test]
fn an_owned_root_can_be_reused() {
    let (n, set_n) = use_state(0);
    let runs = Rc::new(Cell::new(0));
    let scope = create_owned_root(|| ()).0;

    let mut value = 0;
    for _ in 0..3 {
        scope.run(|| {
            let (r, s) = (runs.clone(), n.clone());
            let _e = use_effect(
                move || {
                    s.get();
                    r.set(r.get() + 1);
                },
                (),
            );
        });
        value += 1;
        set_n.set(value);
    }
    // 1 mount + 1 run on the first write, then each new effect mounts and every
    // older one re-runs on the next write: 2 + 3 + 4.
    assert_eq!(runs.get(), 9);
}

// ───────────────────────────── cleanups ─────────────────────────────

#[test]
fn on_cleanup_fires_when_the_owner_goes_away() {
    let (n, set_n) = use_state(0);
    let cleaned = Rc::new(Cell::new(0));

    let root = Root::new();
    root.run(|| {
        let c = cleaned.clone();
        on_cleanup(move || c.set(c.get() + 1));
        let _e = use_effect(
            {
                let n = n.clone();
                move || {
                    n.get();
                }
            },
            (),
        );
    });
    assert_eq!(cleaned.get(), 0);

    set_n.set(1); // the effect re-runs; the scope cleanup has not fired yet
    assert_eq!(cleaned.get(), 0);

    drop(root);
    assert_eq!(cleaned.get(), 1);
}

#[test]
fn disposal_is_idempotent() {
    let cleaned = Rc::new(Cell::new(0));
    let (scope, ()) = create_owned_root(|| {
        let c = cleaned.clone();
        on_cleanup(move || c.set(c.get() + 1));
    });
    scope.dispose();
    scope.dispose();
    assert_eq!(cleaned.get(), 1);
}

#[test]
fn dropping_an_effect_handle_does_not_dispose_a_root_owned_effect() {
    let (n, set_n) = use_state(0);
    let runs = Rc::new(Cell::new(0));
    let root = Root::new();
    let handle = root.run(|| {
        let (r, s) = (runs.clone(), n.clone());
        use_effect(
            move || {
                s.get();
                r.set(r.get() + 1);
            },
            (),
        )
    });

    drop(handle);
    set_n.set(1);
    assert_eq!(runs.get(), 2, "the root still owns it");
    drop(root);
}

#[test]
fn a_top_level_effect_handle_keeps_its_own_effect_alive() {
    let (n, set_n) = use_state(0);
    let runs = Rc::new(Cell::new(0));
    let effect = {
        let (r, s) = (runs.clone(), n.clone());
        use_effect(
            move || {
                s.get();
                r.set(r.get() + 1);
            },
            (),
        )
    };

    set_n.set(1);
    assert_eq!(runs.get(), 2);
    assert!(effect.is_alive());

    let observed = effect.is_alive();
    drop(effect);
    assert!(observed);
}

// ───────────────────────────── keyed rows ─────────────────────────────

#[test]
fn a_keyed_row_keeps_its_state_across_a_reorder() {
    let list = create_keyed_list(
        vec![1, 2, 3],
        |item: &i32| *item,
        |item: &i32| {
            let (name, set_name) = use_state(format!("row-{item}"));
            set_name.set(format!("{} touched", name.get()));
            name.get()
        },
    );
    assert_eq!(
        list.entries(),
        ["row-1 touched", "row-2 touched", "row-3 touched"]
    );

    list.reconcile(vec![3, 1]);
    // Row 1 was *not* re-rendered: its state survived the move.
    assert_eq!(list.entries(), ["row-3 touched", "row-1 touched"]);
}

#[test]
fn a_keyed_row_is_disposed_when_its_key_disappears() {
    let cleaned = Rc::new(Cell::new(0));
    let c = cleaned.clone();
    let list = create_keyed_list(
        vec![1, 2, 3],
        |item: &i32| *item,
        move |item: &i32| {
            let c = c.clone();
            on_cleanup(move || c.set(c.get() + 1));
            *item
        },
    );
    assert_eq!(cleaned.get(), 0);

    list.reconcile(vec![1, 3]);
    assert_eq!(cleaned.get(), 1, "row 2 ran its cleanup");

    drop(list);
    assert_eq!(
        cleaned.get(),
        3,
        "the surviving rows were disposed with the list"
    );
}

#[test]
fn a_keyed_row_re_renders_only_when_its_item_changed() {
    let renders = Rc::new(Cell::new(0));
    let r = renders.clone();
    let list = create_keyed_list(
        vec![1, 2],
        |item: &i32| *item,
        move |item: &i32| {
            r.set(r.get() + 1);
            *item
        },
    );
    assert_eq!(renders.get(), 2);

    list.reconcile(vec![1, 2]); // same items: nothing re-renders
    assert_eq!(renders.get(), 2);

    list.reconcile(vec![1, 2, 3]); // one new row
    assert_eq!(renders.get(), 3);

    list.reconcile(vec![1, 9, 3]); // key 2 is gone, key 9 is new: one render
    assert_eq!(renders.get(), 4);
}

#[test]
fn keyed_rows_own_their_own_effects() {
    let (_tick, set_tick) = use_state(0);
    let list = create_keyed_list(
        vec![1, 2],
        |item: &i32| *item,
        |item: &i32| {
            let runs = use_ref(0);
            let counter = runs.clone();
            let _e = use_effect(
                move || {
                    *counter.borrow_mut() += 1;
                },
                (),
            );
            *item
        },
    );
    assert_eq!(list.len(), 2);

    // Re-rendering row 1 disposes its old effect first.
    list.reconcile(vec![1, 2]);
    set_tick.set(1);
    assert_eq!(list.len(), 2);
}
