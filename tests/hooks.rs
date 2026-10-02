//! Hooks and async data: reducer, previous, resources, keyed rows.

use std::cell::Cell;
use std::rc::Rc;

use signals::*;

// ───────────────────────────── use_reducer ─────────────────────────────

#[derive(Clone, Debug, PartialEq)]
enum Action {
    Add(i32),
    Reset,
}

fn counter_reducer(count: &i32, action: Action) -> i32 {
    match action {
        Action::Add(delta) => count + delta,
        Action::Reset => 0,
    }
}

#[test]
fn a_reducer_folds_actions_into_state() {
    let (count, dispatch) = use_reducer(0, counter_reducer);
    assert_eq!(count.get(), 0);

    dispatch.dispatch(Action::Add(5));
    dispatch.dispatch(Action::Add(3));
    assert_eq!(count.get(), 8);

    dispatch.dispatch(Action::Reset);
    assert_eq!(count.get(), 0);
}

#[test]
fn a_reducer_notifies_effects() {
    let runs = Rc::new(Cell::new(0));
    let (count, dispatch) = use_reducer(0, counter_reducer);
    let _e = use_effect(
        {
            let (r, c) = (runs.clone(), count.clone());
            move || {
                c.get();
                r.set(r.get() + 1);
            }
        },
        (),
    );

    dispatch.dispatch(Action::Add(1));
    dispatch.dispatch(Action::Add(2));
    assert_eq!(runs.get(), 3);
}

#[test]
fn a_reducer_is_a_single_batch_of_work() {
    let seen = Rc::new(Cell::new(0));
    let (count, dispatch) = use_reducer(0, counter_reducer);
    let _e = use_effect(
        {
            let (s, c) = (seen.clone(), count.clone());
            move || s.set(c.get())
        },
        (),
    );

    dispatch.dispatch(Action::Add(4));
    // The effect ran once, and saw the final value.
    assert_eq!(seen.get(), 4);
}

#[test]
fn a_dispatch_can_be_cloned_and_stored() {
    let (count, dispatch) = use_reducer(0, counter_reducer);
    let stored = dispatch.clone();
    stored.dispatch(Action::Add(7));
    assert_eq!(count.get(), 7);
}

// ───────────────────────────── use_previous ─────────────────────────────

#[test]
fn use_previous_reports_the_value_from_before_the_last_run() {
    let (n, set_n) = use_state(1);
    let previous = use_previous(&n);
    let seen = Rc::new(RefCellLog::default());

    let _e = use_effect(
        {
            let (s, n, previous) = (seen.clone(), n.clone(), previous.clone());
            move || {
                let now = n.get();
                s.push(format!("{now} after {:?}", previous.get()));
            }
        },
        (),
    );

    set_n.set(2);
    set_n.set(3);
    assert_eq!(
        seen.lines(),
        ["1 after None", "2 after Some(1)", "3 after Some(2)"]
    );
}

#[test]
fn use_previous_is_lazy() {
    let (n, set_n) = use_state(1);
    let previous = use_previous(&n);
    assert_eq!(previous.peek(), None, "nobody read it yet");

    let snapshot = n.clone();
    let observer = previous.clone();
    let _e = use_effect(
        move || {
            snapshot.get();
            observer.get();
        },
        (),
    );

    set_n.set(2);
    assert_eq!(previous.get(), Some(1));
}

// ───────────────────────────── resources ─────────────────────────────

/// A future that is ready after `n` polls.
struct ReadyAfter {
    left: u8,
    value: &'static str,
}

impl std::future::Future for ReadyAfter {
    type Output = &'static str;
    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<&'static str> {
        if self.left > 0 {
            self.left -= 1;
            cx.waker().wake_by_ref();
            return std::task::Poll::Pending;
        }
        std::task::Poll::Ready(self.value)
    }
}

#[test]
fn a_resource_starts_loading_and_then_becomes_ready() {
    let user = use_resource(|| ReadyAfter {
        left: 1,
        value: "ada",
    });
    assert_eq!(user.get(), ResourceState::Loading);

    tick();
    assert_eq!(user.get(), ResourceState::Ready("ada"));
}

#[test]
fn a_resource_state_can_be_read_as_a_signal() {
    let seen = Rc::new(RefCellLog::default());
    let user = use_resource(|| ReadyAfter {
        left: 0,
        value: "grace",
    });

    let _e = use_effect(
        {
            let (s, state) = (seen.clone(), user.state());
            move || {
                let snapshot = state.get();
                s.push(match snapshot {
                    ResourceState::Loading => "loading".into(),
                    ResourceState::Ready(value) => format!("ready {value}"),
                    other => format!("{other:?}"),
                });
            }
        },
        (),
    );

    assert_eq!(seen.lines(), ["loading"]);
    tick();
    assert_eq!(seen.lines(), ["loading", "ready grace"]);
}

#[test]
fn refetching_throws_the_old_value_away() {
    let calls = Rc::new(Cell::new(0));
    let counter = calls.clone();
    let user = use_resource(move || {
        let n = counter.get();
        counter.set(n + 1);
        ReadyAfter {
            left: 0,
            value: if n == 0 { "first" } else { "second" },
        }
    });

    tick();
    assert_eq!(user.peek(), ResourceState::Ready("first"));

    user.refetch();
    assert_eq!(user.peek(), ResourceState::Loading, "loading again");
    tick();
    assert_eq!(user.peek(), ResourceState::Ready("second"));
    assert_eq!(calls.get(), 2);
}

#[test]
fn a_disposed_resource_never_writes() {
    let (value, set_value) = use_state(String::from("before"));
    let sink = set_value.clone();

    let resource = create_root(|| {
        let sink = sink.clone();
        use_resource(move || {
            let sink = sink.clone();
            async move {
                // A future that is ready but asks to be polled again, so the
                // resource is still mid-fetch when the scope is disposed.
                struct Once;
                impl std::future::Future for Once {
                    type Output = String;
                    fn poll(
                        self: std::pin::Pin<&mut Self>,
                        cx: &mut std::task::Context<'_>,
                    ) -> std::task::Poll<String> {
                        cx.waker().wake_by_ref();
                        std::task::Poll::Ready("after".into())
                    }
                }
                sink.set(Once.await)
            }
        })
    });

    assert_eq!(
        value.get(),
        "before",
        "nothing yet: the future has not been polled"
    );
    drop(resource);
    tick();
    assert_eq!(value.get(), "before", "a disposed resource stays silent");
}

#[test]
fn a_resource_can_project_its_value() {
    let id = use_resource_with(
        || ReadyAfter {
            left: 0,
            value: "ada",
        },
        |value| value.len(),
    );
    tick();
    assert_eq!(id.get(), ResourceState::Ready(3));
}

#[test]
fn resource_state_helpers_describe_the_state() {
    let loading = ResourceState::<u8>::Loading;
    assert!(loading.is_loading());
    assert!(!loading.is_idle());
    assert_eq!(loading.value(), None);
    assert_eq!(loading.error(), None);

    let ready = ResourceState::Ready(7u8);
    assert_eq!(ready.value(), Some(&7));

    let failed = ResourceState::<u8>::Failed("boom".into());
    assert_eq!(failed.error(), Some("boom"));
    assert_eq!(
        failed.clone().map(|v| v * 2),
        ResourceState::Failed("boom".into())
    );

    assert!(ResourceState::<u8>::Idle.is_idle());
}

// ───────────────────────────── keyed list ─────────────────────────────

#[test]
fn a_keyed_list_renders_rows_in_order() {
    let list = create_keyed_list(
        vec!["a", "b"],
        |item: &&str| item.to_string(),
        |item: &&str| format!("<{item}>"),
    );
    assert_eq!(list.entries(), ["<a>", "<b>"]);
    assert_eq!(list.len(), 2);
    assert!(!list.is_empty());
    assert_eq!(list.get(&"a".to_string()), Some("<a>".into()));
    assert_eq!(list.get(&"zzz".to_string()), None);
}

#[test]
fn a_keyed_list_reorders_without_re_rendering() {
    let renders = Rc::new(Cell::new(0));
    let counter = renders.clone();
    let list = create_keyed_list(
        vec![1, 2, 3],
        |item: &i32| *item,
        move |item: &i32| {
            counter.set(counter.get() + 1);
            item.to_string()
        },
    );
    assert_eq!(renders.get(), 3);

    list.reconcile(vec![3, 2, 1]);
    assert_eq!(renders.get(), 3, "a reorder is not a re-render");
    assert_eq!(list.entries(), ["3", "2", "1"]);
    assert_eq!(list.keys(), [3, 2, 1]);
}

#[test]
fn an_empty_keyed_list_is_empty() {
    let list = create_keyed_list(Vec::<i32>::new(), |item: &i32| *item, |item: &i32| *item);
    assert!(list.is_empty());
    assert_eq!(list.len(), 0);

    list.reconcile(vec![1, 2]);
    assert_eq!(list.len(), 2);

    list.reconcile(Vec::new());
    assert!(list.is_empty());
}

#[test]
fn a_keyed_row_can_react_to_signals() {
    let (selected, set_selected) = use_state(0);
    let watch = selected.clone();
    let highlighted = Rc::new(std::cell::RefCell::new(Vec::new()));
    let sink_all = highlighted.clone();

    let list = create_keyed_list(
        vec![1, 2, 3],
        |item: &i32| *item,
        move |item: &i32| {
            let key = *item;
            let watch = watch.clone();
            let sink = sink_all.clone();
            let _effect = use_effect(
                move || {
                    if watch.get() == key {
                        sink.borrow_mut().push(key);
                    }
                },
                (),
            );
            key
        },
    );
    assert_eq!(list.len(), 3);
    assert!(highlighted.borrow().is_empty());

    // Each row keeps its own subscription: only the matching row reacts.
    set_selected.set(2);
    assert_eq!(*highlighted.borrow(), [2]);

    set_selected.set(3);
    assert_eq!(*highlighted.borrow(), [2, 3]);
}

// ───────────────────────────── helper ─────────────────────────────

/// A tiny append-only log that clones cheaply.
#[derive(Clone, Default)]
struct RefCellLog(Rc<std::cell::RefCell<Vec<String>>>);

impl RefCellLog {
    fn push(&self, line: String) {
        self.0.borrow_mut().push(line);
    }
    fn lines(&self) -> Vec<String> {
        self.0.borrow().clone()
    }
}
