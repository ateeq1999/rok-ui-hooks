//! State, memos, effects and cleanup, printed step by step.
//!
//! ```text
//! cargo run --example demo
//! ```
//!
//! `docs/getting-started.md` covers the same ground in prose.

use signals::*;

fn main() {
    println!("1. Create");
    let (first, _set_first) = use_state("John".to_string());
    let (last, set_last) = use_state("Smith".to_string());
    let (show_full, set_show_full) = use_state(true);

    let display = use_memo(
        {
            let (first, last, show_full) = (first.clone(), last.clone(), show_full.clone());
            move || {
                println!("   ### executing display_name");
                let p = |line: &str| println!("   ### {line}");
                use_cleanup(move || p("releasing display_name dependencies"));
                if !show_full.get() {
                    return first.get();
                }
                format!("{} {}", first.get(), last.get())
            }
        },
        (),
    );

    let _effect = use_effect(
        {
            let display = display.clone();
            move || println!("My name is {}", display.get())
        },
        (),
    );

    println!("2. Set show_full: false");
    set_show_full.set(false);
    println!("3. Change last name");
    set_last.set("Legend".into());
    println!("4. Set show_full: true");
    set_show_full.set(true);
}
