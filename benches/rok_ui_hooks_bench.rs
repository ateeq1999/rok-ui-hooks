//! Zero-dependency benchmarks. Run with `cargo bench`.
//!
//! These are not criterion numbers — they are a reproducible smoke test that the
//! cost model of the engine holds: reads are cheap, writes are O(subscribers), and
//! a diamond recomputes its memos once per change, not once per reader.

use std::time::{Duration, Instant};

use rok_ui_hooks::*;

const ROUNDS: u32 = 20_000;

fn time(label: &str, rounds: u32, mut body: impl FnMut()) {
    // Warm up, then measure.
    for _ in 0..rounds / 4 {
        body();
    }
    let start = Instant::now();
    for _ in 0..rounds {
        body();
    }
    let elapsed = start.elapsed();
    let per_op = elapsed / rounds;
    println!("{label:<44} {:>10.1?} total {:>12.1?}/op", elapsed, per_op);
}

fn bench_signal_writes() {
    let (a, set_a) = use_state(0u64);
    time("signal write (no subscribers)", ROUNDS, || {
        set_a.set(a.get_untracked() + 1);
    });
}

fn bench_signal_reads() {
    let (a, _) = use_state(0u64);
    time("signal read", ROUNDS * 10, || {
        a.get_untracked();
    });
}

fn bench_effect_runs() {
    let (a, set_a) = use_state(0u64);
    let (b, set_b) = use_state(0u64);
    let sink = use_ref(0u64);
    let _e = use_effect(
        {
            let (a, b, sink) = (a.clone(), b.clone(), sink.clone());
            move || {
                let sum = a.get() + b.get();
                *sink.borrow_mut() = sum;
            }
        },
        (),
    );
    time("write → 1 effect run", ROUNDS, || {
        set_a.set(a.get_untracked() + 1);
    });
    time("batch of 2 writes → 1 effect run", ROUNDS / 10, || {
        batch(|| {
            set_a.set(a.get_untracked() + 1);
            set_b.set(b.get_untracked() + 1);
        });
    });
}

fn bench_effect_count() {
    for effects in [1usize, 10, 100, 1_000] {
        let (a, set_a) = use_state(0u64);
        let sink = use_ref(0u64);
        let root = Root::new();
        root.run(|| {
            for _ in 0..effects {
                let a = a.clone();
                let sink = sink.clone();
                let _e = use_effect(
                    move || {
                        let value = a.get();
                        *sink.borrow_mut() += value;
                    },
                    (),
                );
            }
        });
        let per_write = measure(|| set_a.set(a.get_untracked() + 1));
        println!(
            "fan-out: 1 write → {effects:>5} effects       {:>12.1?}/write",
            per_write
        );
    }
}

fn measure(mut body: impl FnMut()) -> Duration {
    body();
    let start = Instant::now();
    for _ in 0..1_000 {
        body();
    }
    start.elapsed() / 1_000
}

fn bench_diamond() {
    let (a, set_a) = use_state(1u64);
    let a2 = a.clone();
    let double = use_memo(
        {
            let a = a2.clone();
            move || a.get() * 2
        },
        (a2,),
    );
    let a3 = a.clone();
    let triple = use_memo(move || a3.get() * 3, ());
    let (double2, triple2) = (double.clone(), triple.clone());
    let sink = use_ref(0u64);
    let _e = use_effect(
        {
            let sink = sink.clone();
            move || {
                *sink.borrow_mut() = double2.get() + triple2.get();
            }
        },
        (),
    );
    time("diamond: 2 memos, 1 reader", ROUNDS, || {
        set_a.set(a.get_untracked() + 1);
    });
}

fn bench_store() {
    let store = create_store(0u64);
    time("store update (no subscribers)", ROUNDS, || {
        store.update(|v| *v += 1);
    });
}

fn main() {
    println!("signals — {} rounds per measurement\n", ROUNDS);
    bench_signal_reads();
    bench_signal_writes();
    bench_effect_runs();
    bench_effect_count();
    bench_diamond();
    bench_store();
}
