//! Scheduling: batching, deferred effects, flushing and `tick`.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use signals::*;

// ───────────────────────────── batch ─────────────────────────────

#[test]
fn a_batch_runs_each_effect_once() {
    let runs = Rc::new(Cell::new(0));
    let (n, set_n) = use_state(0);
    let _e = use_effect(
        {
            let (r, s) = (runs.clone(), n.clone());
            move || {
                s.get();
                r.set(r.get() + 1);
            }
        },
        (),
    );

    batch(|| {
        set_n.set(1);
        set_n.set(2);
        set_n.set(3);
    });
    assert_eq!(runs.get(), 2, "mount + one run for the whole batch");

    // Outside a batch every write is its own flush.
    set_n.set(4);
    assert_eq!(runs.get(), 3);
}

#[test]
fn a_batch_returns_the_value_of_its_body() {
    let answer = batch(|| 6 * 7);
    assert_eq!(answer, 42);
}

#[test]
fn nested_batches_flush_once() {
    let runs = Rc::new(Cell::new(0));
    let (n, set_n) = use_state(0);
    let _e = use_effect(
        {
            let (r, s) = (runs.clone(), n.clone());
            move || {
                s.get();
                r.set(r.get() + 1);
            }
        },
        (),
    );

    batch(|| {
        set_n.set(1);
        batch(|| set_n.set(2));
        assert_eq!(runs.get(), 1, "the inner batch does not flush");
    });
    assert_eq!(runs.get(), 2);
}

// ───────────────────────────── deferred effects ─────────────────────────────

#[test]
fn a_deferred_effect_waits_for_flush() {
    let runs = Rc::new(Cell::new(0));
    let (n, set_n) = use_state(0);
    let _e = create_deferred_effect(
        {
            let (r, s) = (runs.clone(), n.clone());
            move || {
                s.get();
                r.set(r.get() + 1);
            }
        },
        (),
    );
    assert_eq!(runs.get(), 1, "mount runs immediately");

    set_n.set(1);
    set_n.set(2);
    assert_eq!(runs.get(), 1, "queued, not run");

    flush();
    assert_eq!(runs.get(), 2, "one run for both writes");

    set_n.set(3);
    flush();
    assert_eq!(runs.get(), 3);
}

#[test]
fn flush_runs_eager_effects_too() {
    let eager = Rc::new(Cell::new(0));
    let (n, set_n) = use_state(0);
    let _e = use_effect(
        {
            let (r, s) = (eager.clone(), n.clone());
            move || {
                s.get();
                r.set(r.get() + 1);
            }
        },
        (),
    );
    let _deferred = create_deferred_effect(
        {
            let s = n.clone();
            move || {
                s.get();
            }
        },
        (),
    );

    set_n.set(1);
    assert_eq!(eager.get(), 2, "eager effects already ran on the write");
    flush();
}

#[test]
fn a_flush_with_nothing_queued_is_a_no_op() {
    flush();
    flush();
}

// ───────────────────────────── tick ─────────────────────────────

#[test]
fn tick_runs_deferred_effects() {
    let runs = Rc::new(Cell::new(0));
    let (n, set_n) = use_state(0);
    let _e = create_deferred_effect(
        {
            let (r, s) = (runs.clone(), n.clone());
            move || {
                s.get();
                r.set(r.get() + 1);
            }
        },
        (),
    );

    set_n.set(1);
    assert_eq!(runs.get(), 1);
    tick();
    assert_eq!(runs.get(), 2);
}

#[test]
fn tick_polls_spawned_futures() {
    let (text, set_text) = use_state(String::from("loading"));
    let signal = set_text.clone();
    let _task = spawn(async move {
        signal.set("loaded".into());
    });

    assert_eq!(text.get(), "loading", "nothing runs before the first tick");
    tick();
    assert_eq!(text.get(), "loaded");
}

#[test]
fn a_task_is_polled_until_it_completes() {
    let steps = Rc::new(Cell::new(0));
    let counter = steps.clone();
    let _task = spawn(async move {
        counter.set(counter.get() + 1);
    });

    assert_eq!(steps.get(), 0);
    tick();
    assert_eq!(steps.get(), 1);
    assert_eq!(pending_tasks(), 0, "a finished task is cleaned up");
}

#[test]
fn a_yielding_future_is_polled_until_it_completes() {
    /// A future that needs several polls before it is ready.
    struct Slow {
        left: u8,
    }
    impl std::future::Future for Slow {
        type Output = u8;
        fn poll(
            mut self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<u8> {
            if self.left > 0 {
                self.left -= 1;
                cx.waker().wake_by_ref();
                return std::task::Poll::Pending;
            }
            std::task::Poll::Ready(42)
        }
    }

    let (answer, set_answer) = use_state(0u8);
    let sink = set_answer.clone();
    let _task = spawn(async move {
        sink.set(Slow { left: 3 }.await);
    });

    assert_eq!(answer.get(), 0, "nothing before the first tick");
    tick(); // one tick drains the task, however many polls it needs
    assert_eq!(answer.get(), 42);
    assert_eq!(pending_tasks(), 0);
}

#[test]
fn a_future_can_write_signals_and_read_them_back() {
    let (n, set_n) = use_state(1);
    let write = set_n.clone();
    let read = n.clone();
    let _task = spawn(async move {
        let doubled = read.get() * 2;
        write.set(doubled + 1);
    });
    tick();
    assert_eq!(n.get(), 3);
}

#[test]
fn dropping_a_task_cancels_it() {
    let steps = Rc::new(Cell::new(0));
    let counter = steps.clone();
    let task = spawn(async move {
        counter.set(counter.get() + 1);
    });
    drop(task);
    tick();
    assert_eq!(steps.get(), 0, "a dropped task is never polled");
}

#[test]
fn a_deferred_effect_mounts_immediately_but_waits_to_update() {
    let runs = Rc::new(Cell::new(0));
    let _effect = create_deferred_effect(
        {
            let r = runs.clone();
            move || {
                r.set(r.get() + 1);
            }
        },
        (),
    );
    assert_eq!(runs.get(), 1, "mount is immediate, even when deferred");
}

#[test]
fn a_debounced_value_lands_after_the_quiet_period() {
    let (text, set_text) = use_state(String::from("idle"));
    let debounced = use_debounced(&text, Duration::from_millis(10));

    assert_eq!(debounced.get(), "idle");

    // A burst of writes: only the last one may survive.
    set_text.set("ty".into());
    tick();
    set_text.set("typed".into());

    for _ in 0..40 {
        std::thread::sleep(Duration::from_millis(5));
        tick();
        if debounced.get() == "typed" {
            return;
        }
    }
    panic!("the trailing value never landed");
}

#[test]
fn a_debounced_value_ignores_an_intermediate_write() {
    let (text, set_text) = use_state(String::from("idle"));
    let debounced = use_debounced(&text, Duration::from_millis(20));

    set_text.set("first".into());
    for _ in 0..20 {
        std::thread::sleep(Duration::from_millis(5));
        tick();
        if debounced.get() == "first" {
            break;
        }
    }
    assert_eq!(debounced.get(), "first");

    // Now write twice, quickly: the second cancels the first timer.
    set_text.set("second".into());
    set_text.set("third".into());
    for _ in 0..40 {
        std::thread::sleep(Duration::from_millis(5));
        tick();
        if debounced.get() == "third" {
            break;
        }
    }
    assert_eq!(debounced.get(), "third", "\"second\" was cancelled");
}

#[test]
fn a_throttled_value_leads_and_then_trails() {
    let (text, set_text) = use_state(String::from("idle"));
    let throttled = use_throttled(&text, Duration::from_millis(50));
    assert_eq!(throttled.get(), "idle");

    set_text.set("first".into());
    assert_eq!(throttled.get(), "first", "the leading edge is immediate");

    // Inside the window: nothing goes through yet.
    set_text.set("second".into());
    set_text.set("third".into());
    assert_eq!(throttled.get(), "first");

    for _ in 0..60 {
        std::thread::sleep(Duration::from_millis(5));
        tick();
        if throttled.get() == "third" {
            break;
        }
    }
    assert_eq!(
        throttled.get(),
        "third",
        "the trailing edge carries the last value"
    );
}
#[test]
fn a_scope_owns_the_futures_spawned_inside_it() {
    let ran = Rc::new(Cell::new(false));

    let root = Root::new();
    root.run(|| {
        let ran = ran.clone();
        let task = spawn(async move {
            ran.set(true);
        });
        drop(task); // the scope still owns the future
    });

    assert!(!ran.get(), "a task is never polled outside tick");
    tick();
    assert!(ran.get(), "the scope owned it, so it ran anyway");

    root.dispose();
    tick();
}

#[test]
fn a_disposed_scope_cancels_its_futures() {
    let polls = Rc::new(Cell::new(0));

    let root = Root::new();
    root.run(|| {
        let polls = polls.clone();
        let _task = spawn(async move {
            std::future::poll_fn(move |cx| {
                polls.set(polls.get() + 1);
                cx.waker().wake_by_ref();
                std::task::Poll::<()>::Pending
            })
            .await
        });
    });

    tick();
    assert!(polls.get() > 0, "a waking future is polled inside tick");

    root.dispose();
    let after = polls.get();
    tick();
    assert_eq!(polls.get(), after, "a disposed scope cancels its futures");
}
