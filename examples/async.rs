//! Futures, resources, timers, and the tick loop.
//!
//! ```text
//! cargo run --example async
//! ```
//!
//! There is no background thread: everything runs when you call [`tick`], so
//! nothing here needs `Send` and nothing can mutate the graph off-thread.

use signals::*;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;

/// Stands in for a request: `Pending` for the first `skip` polls, then done.
struct Later {
    skip: u8,
    polls: Rc<AtomicUsize>,
    value: String,
}

impl Future for Later {
    type Output = String;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<String> {
        self.polls.fetch_add(1, Ordering::Relaxed);
        if self.skip == 0 {
            return Poll::Ready(std::mem::take(&mut self.value));
        }
        self.skip -= 1;
        // Waking immediately lets the executor finish this future inside the
        // same tick. A future woken by real I/O would simply wait for the next.
        cx.waker().wake_by_ref();
        Poll::Pending
    }
}

fn report(resource: &Resource<String>) {
    match resource.peek() {
        ResourceState::Idle => println!("   idle"),
        ResourceState::Loading => println!("   loading…"),
        ResourceState::Ready(value) => println!("   ready: {value}"),
        ResourceState::Failed(error) => println!("   failed: {error}"),
    }
}

fn main() {
    println!("1. A resource is Loading until its future resolves");
    let polls = Rc::new(AtomicUsize::new(0));
    let scope = Root::new();
    scope.run(|| {
        let polls = polls.clone();
        let profile = use_resource(move || Later {
            skip: 2,
            polls: polls.clone(),
            value: "user-1".into(),
        });

        // Reading the state inside an effect is what makes the UI react.
        let _effect = use_effect(
            {
                let profile = profile.clone();
                move || report(&profile)
            },
            (),
        );

        println!("   right after the fetch started");
        report(&profile);
        tick();
        println!("   after one tick");
        report(&profile);
    });
    println!("   polls so far: {}", polls.load(Ordering::Relaxed));

    println!("\n2. `refetch` throws the old value away");
    scope.run(|| {
        let polls = polls.clone();
        let profile = use_resource(move || Later {
            skip: 1,
            polls: polls.clone(),
            value: "user-2".into(),
        });
        let _effect = use_effect(
            {
                let profile = profile.clone();
                move || report(&profile)
            },
            (),
        );
        println!("   created fresh");
        report(&profile);
        profile.refetch();
        println!("   immediately after refetch");
        report(&profile);
        tick();
        println!("   after one tick");
        report(&profile);
    });
    println!("   polls so far: {}", polls.load(Ordering::Relaxed));

    println!("\n3. A timer fires on the next `tick` at or after its deadline");
    let (saved, set_saved) = use_state(false);
    let debounced = use_debounced(&saved, Duration::from_millis(5));
    let _effect = use_effect(
        {
            let settled = debounced.read_signal();
            move || println!("   observer saw {settled}", settled = settled.get())
        },
        (),
    );
    set_saved.set(true);
    println!("   right after the write:  {}", debounced.get());
    std::thread::sleep(Duration::from_millis(10));
    println!("   after sleeping:         {}", debounced.get());
    tick();
    println!("   after tick:             {}", debounced.get());

    println!("\n4. A throttle keeps the first change of each window");
    let runs = Rc::new(AtomicUsize::new(0));
    let (signal, set_signal) = use_state(0u32);
    let throttled = use_throttled(&signal, Duration::from_millis(50));
    let settled = throttled.read_signal();
    let _effect = use_effect(
        {
            let settled = settled.clone();
            let runs = runs.clone();
            move || {
                runs.fetch_add(1, Ordering::Relaxed);
                println!("   observer saw {}", settled.get());
            }
        },
        (),
    );
    for i in 1..=4 {
        set_signal.set(i);
        std::thread::sleep(Duration::from_millis(2));
        tick();
    }
    println!("   observer ran {} time(s)", runs.load(Ordering::Relaxed));
    std::thread::sleep(Duration::from_millis(60));
    tick();
    println!(
        "   after the window closes: {} (ran {} time(s))",
        settled.get(),
        runs.load(Ordering::Relaxed)
    );

    println!("\n5. Disposing a scope cancels what is still in flight");
    let late = Rc::new(AtomicUsize::new(0));
    let doomed = Root::new();
    doomed.run(|| {
        let late = late.clone();
        let _task = spawn(async move {
            // Never wakes itself, so the executor polls it once per tick.
            std::future::poll_fn(move |_cx| {
                late.fetch_add(1, Ordering::Relaxed);
                Poll::<()>::Pending
            })
            .await
        });
    });
    tick();
    let polled = late.load(Ordering::Relaxed);
    println!("   polled {polled} time(s) while alive");
    drop(doomed);
    let after = late.load(Ordering::Relaxed);
    tick();
    println!(
        "   after disposal: {polled} → {} (the task is gone)",
        late.load(Ordering::Relaxed)
    );
    assert_eq!(after, polled, "a disposed task must not be polled again");

    println!("\n6. The resource survives as long as its scope");
    drop(scope);
    println!("   done");
}
