//! A small but complete app: a todo list whose state lives in a store, is
//! filtered by a memo, and is rendered by "components" that are really just
//! effects.
//!
//! ```text
//! cargo run --example todo_app
//! ```

use rok_ui_hooks::*;

#[derive(Clone, PartialEq)]
struct Todo {
    title: String,
    done: bool,
}

#[derive(Clone)]
struct App {
    todos: Vec<Todo>,
    draft: String,
    filter: Filter,
}

#[derive(Clone, Copy, PartialEq)]
enum Filter {
    All,
    Pending,
    Done,
}

impl Filter {
    fn label(self) -> &'static str {
        match self {
            Filter::All => "all",
            Filter::Pending => "pending",
            Filter::Done => "done",
        }
    }
}

impl App {
    fn visible(&self) -> Vec<&Todo> {
        self.todos
            .iter()
            .filter(|t| match self.filter {
                Filter::All => true,
                Filter::Pending => !t.done,
                Filter::Done => t.done,
            })
            .collect()
    }
}

fn main() {
    let app = create_store(App {
        todos: vec![
            Todo {
                title: "Learn signals".into(),
                done: true,
            },
            Todo {
                title: "Write an app".into(),
                done: false,
            },
        ],
        draft: String::new(),
        filter: Filter::All,
    });

    // ── "components" ─────────────────────────────────────────────────────────
    // Each is a hook that runs once and re-runs exactly when the state it
    // declares actually changes.

    let title_bar = use_effect(
        {
            let app = app.clone();
            move || {
                let done = use_store(&app, |a| a.todos.iter().filter(|t| t.done).count());
                let all = use_store(&app, |a| a.todos.len());
                println!("\nMy Todos ({done}/{all} complete)");
            }
        },
        (),
    );

    let visible = app.select(|a: &App| {
        a.visible()
            .into_iter()
            .map(|t| (t.title.clone(), t.done))
            .collect::<Vec<_>>()
    });

    let todo_list = use_effect(
        {
            let (app, visible) = (app.clone(), visible.clone());
            move || {
                for (title, done) in visible.get() {
                    println!("  [{}] {title}", if done { 'x' } else { ' ' });
                }
                println!("  filter: {}", use_store(&app, |a| a.filter.label()));
            }
        },
        (visible,),
    );

    let status = use_effect(
        {
            let app = app.clone();
            move || {
                let draft = use_store(&app, |a| a.draft.clone());
                println!(
                    "  draft: {draft:?} + filter buttons [{}]",
                    use_store(&app, |a| a.filter.label())
                );
            }
        },
        (),
    );

    // ── actions ───────────────────────────────────────────────────────────────

    println!("\n== initial render ==");
    let _ = title_bar;

    println!("\n== typing a draft ==");
    app.update(|a| a.draft.push_str("Ship "));
    app.update(|a| a.draft.push_str("it"));

    println!("\n== adding the draft ==");
    app.update(|a| {
        let title = std::mem::take(&mut a.draft).trim().to_string();
        if !title.is_empty() {
            a.todos.push(Todo { title, done: false });
        }
    });

    println!("\n== toggling the first todo ==");
    app.update(|a| a.todos[0].done = false);

    println!("\n== switching the filter to 'done' ==");
    app.update(|a| a.filter = Filter::Done);
    println!("  (the visible list recomputed: the selector value changed)");

    println!("\n== switching the filter to 'pending' ==");
    app.update(|a| a.filter = Filter::Pending);

    println!("\n== a mutation the view does not depend on ==");
    let draft_before = app.with(|a| a.draft.clone());
    app.update(|a| a.draft.push_str("(ignored)"));
    assert_ne!(draft_before, app.with(|a| a.draft.clone()));
    println!(
        "  draft changed to {:?}, but the list and the title bar stayed put",
        app.with(|a| a.draft.clone())
    );

    drop((todo_list, status));

    println!("\n-- unmounted: writes no longer render anything --");
    app.update(|a| a.filter = Filter::All);
    println!("done.");
}
