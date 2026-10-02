//! The Zustand-style store: tracked reads, selectors, listeners.
//!
//! Convention used throughout: a store needed by both an effect body and the
//! test body itself is cloned for the effect.

mod common;

use common::log;
use rok_ui_hooks::*;

#[derive(Clone, Debug, PartialEq)]
struct App {
    count: i32,
    name: String,
}

fn app() -> App {
    App {
        count: 0,
        name: "Ada".into(),
    }
}

#[test]
fn get_set_update() {
    let store = create_store(app());
    assert_eq!(store.with(|s| s.count), 0);
    store.update(|s| s.count += 1);
    assert_eq!(store.with(|s| s.count), 1);
    store.set(App {
        count: 10,
        name: "Bob".into(),
    });
    assert_eq!(
        store.get(),
        App {
            count: 10,
            name: "Bob".into()
        }
    );
}

#[test]
fn use_store_is_a_tracked_selector_read() {
    let (logs, push) = log();
    let store = create_store(app());
    let _e = use_effect(
        {
            let (store, p) = (store.clone(), push.clone());
            move || p(format!("{}", use_store(&store, |s| s.count)))
        },
        (),
    );
    store.update(|s| s.count = 1);
    store.update(|s| s.count = 2);
    assert_eq!(*logs.borrow(), ["0", "1", "2"]);
}

#[test]
fn use_store_respects_declared_deps() {
    let (logs, push) = log();
    let store = create_store(app());
    let name = store.select(|s: &App| s.name.clone());
    let _e = use_effect(
        {
            let (store, name, p) = (store.clone(), name.clone(), push.clone());
            move || {
                let count = use_store(&store, |s| s.count);
                p(format!("{} has {}", name.get(), count))
            }
        },
        (name,),
    );
    store.update(|s| s.count += 1); // `name` unchanged → bail out
    assert_eq!(*logs.borrow(), ["Ada has 0"]);
    store.update(|s| s.name = "Grace".into());
    assert_eq!(*logs.borrow(), ["Ada has 0", "Grace has 1"]);
}

#[test]
fn select_caches_unchanged_slices() {
    let (logs, push) = log();
    let store = create_store(app());
    let name = store.select(|s: &App| s.name.clone());
    let _e = use_effect(
        {
            let (name, p) = (name.clone(), push.clone());
            move || p(name.get())
        },
        (name,),
    );
    store.update(|s| s.count = 5); // memo recomputes, value identical → no notify
    assert_eq!(*logs.borrow(), ["Ada"]);
    store.update(|s| s.name = "Grace".into());
    assert_eq!(*logs.borrow(), ["Ada", "Grace"]);
}

#[test]
fn subscribe_reports_new_and_previous_state() {
    let (logs, push) = log();
    let store = create_store(0i32);
    let sub = store.subscribe(move |next: &i32, prev: &i32| push(format!("{prev}->{next}")));
    store.set(1);
    store.update(|v| *v += 1);
    drop(sub);
    store.set(9); // unsubscribed
    assert_eq!(*logs.borrow(), ["0->1", "1->2"]);
}

#[test]
fn store_reacts_inside_batch() {
    let (logs, push) = log();
    let store = create_store(app());
    let _e = use_effect(
        {
            let (store, p) = (store.clone(), push.clone());
            move || {
                let name = use_store(&store, |s| s.name.clone());
                let count = use_store(&store, |s| s.count);
                p(format!("{name} / {count}"))
            }
        },
        (),
    );
    batch(|| {
        store.update(|s| s.name = "Grace".into());
        store.update(|s| s.count = 1);
    });
    assert_eq!(*logs.borrow(), ["Ada / 0", "Grace / 1"]);
}

#[test]
fn untracked_reads_do_not_subscribe() {
    let (logs, push) = log();
    let store = create_store(app());
    let _e = use_effect(
        {
            let (store, p) = (store.clone(), push.clone());
            move || p(format!("{}", store.with_untracked(|s| s.count)))
        },
        (),
    );
    store.update(|s| s.count = 3);
    assert_eq!(*logs.borrow(), ["0"]);
}
