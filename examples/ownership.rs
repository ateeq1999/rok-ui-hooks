//! Ownership: who disposes what, and when.
//!
//! ```text
//! cargo run --example ownership
//! ```

use signals::*;

fn main() {
    println!("1. A scope owns what is created inside it");
    let (count, set_count) = use_state(0);

    let root = Root::new();
    root.run(|| {
        let count = count.clone();
        let _effect = use_effect(
            move || {
                println!("   effect saw count = {}", count.get());
            },
            (),
        );
    });

    set_count.set(1);
    println!("   the effect handle is gone, but the root still owns it");

    println!("\n2. Dropping the root disposes it");
    drop(root);
    set_count.set(2);
    println!("   no output above: the effect was disposed with its scope");

    println!("\n3. Cleanups run on disposal");
    {
        let _root = Root::new();
        _root.run(|| {
            on_cleanup(|| println!("   scope cleanup"));
            let _effect = use_effect(
                {
                    let count = count.clone();
                    move || {
                        let _ = count.get();
                        println!("   effect run");
                    }
                },
                (),
            );
        });
        set_count.set(3);
        println!("   the scope cleanup has not fired yet");
    }
    println!("   …and now it has");

    println!("\n4. An owned root can be handed around and reused");
    let (n, set_n) = use_state(0);
    let (scope, ()) = create_owned_root(|| {
        let n = n.clone();
        let _effect = use_effect(move || println!("   n = {}", n.get()), ());
    });
    set_n.set(1);
    scope.run(|| {
        let n = n.clone();
        let _effect = use_effect(move || println!("   second listener: n = {}", n.get()), ());
    });
    set_n.set(2);
    scope.dispose();
    set_n.set(3);
    println!("   nothing after dispose");

    println!("\n5. Nested computations die with the run that made them");
    let (label, set_label) = use_state("outer".to_string());
    let root = Root::new();
    root.run(|| {
        let label = label.clone();
        let _effect = use_effect(
            move || {
                let inner = use_effect(
                    {
                        let label = label.clone();
                        move || println!("   inner: {}", label.get())
                    },
                    (),
                );
                use_cleanup(move || drop(inner));
                println!("outer: {}", label.get());
            },
            (),
        );
    });
    set_label.set("changed".into());
}
