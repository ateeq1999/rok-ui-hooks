//! Keyed lists: reconcile by identity, not by position.
//!
//! ```text
//! cargo run --example keyed_list
//! ```

use rok_ui_hooks::*;

#[derive(Clone, PartialEq, Debug)]
struct Task {
    id: u32,
    title: String,
    done: bool,
}

fn main() {
    println!("1. Render three rows");
    let list = create_keyed_list(
        vec![
            Task {
                id: 1,
                title: "write".into(),
                done: false,
            },
            Task {
                id: 2,
                title: "review".into(),
                done: false,
            },
            Task {
                id: 3,
                title: "ship".into(),
                done: false,
            },
        ],
        |task: &Task| task.id,
        |task: &Task| {
            // The render callback borrows the item, so a nested computation —
            // which must be `'static` — copies out what it needs.
            let (id, title, done) = (task.id, task.title.clone(), task.done);

            // A row may own state: it lives exactly as long as its key.
            let (note, set_note) = use_state(String::new());
            let _effect = use_effect(
                {
                    let note = note.clone();
                    move || {
                        if note.get().is_empty() {
                            println!("   row {id} mounted");
                        }
                    }
                },
                (),
            );
            on_cleanup(move || println!("   row {id} cleaned up"));
            let _ = set_note;
            format!("[{}] {}", if done { "x" } else { " " }, title)
        },
    );
    println!("   {:?}", list.entries());

    println!("\n2. Toggle row 1 — a reorder plus a change");
    list.reconcile(vec![
        Task {
            id: 3,
            title: "ship".into(),
            done: true,
        },
        Task {
            id: 1,
            title: "write".into(),
            done: true,
        },
        Task {
            id: 2,
            title: "review".into(),
            done: false,
        },
    ]);
    println!("   {:?}", list.entries());
    println!("   keys in order: {:?}", list.keys());

    println!("\n3. Row 2 was not re-rendered: it only moved");
    println!("   rows 3 and 1 changed, so they were re-rendered in place");

    println!("\n4. Removing key 2 disposes exactly that row");
    list.reconcile(vec![
        Task {
            id: 3,
            title: "ship".into(),
            done: true,
        },
        Task {
            id: 1,
            title: "write".into(),
            done: true,
        },
    ]);
    println!("   {:?} (len = {})", list.entries(), list.len());

    println!("\n5. Dropping the list disposes the rest");
    drop(list);
    println!("   done");
}
