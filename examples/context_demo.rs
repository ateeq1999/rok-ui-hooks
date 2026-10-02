//! The Context API: a typed, reactive value you can provide down a subtree.
//!
//! ```text
//! cargo run --example context_demo
//! ```

use signals::*;

#[derive(Clone)]
enum Theme {
    Light,
    Dark,
    HighContrast,
}

impl Theme {
    fn name(&self) -> &'static str {
        match self {
            Theme::Light => "light",
            Theme::Dark => "dark",
            Theme::HighContrast => "high-contrast",
        }
    }
}

/// A context has to be created once, outside any computation, and shared.
fn main() {
    let theme = create_context(Theme::Light);
    println!("no provider        → {}", theme.get().name());

    app(&theme);
}

/// A miniature component tree: `App` renders `Toolbar`, `Sidebar` and `Panel`.
fn app(theme: &Context<Theme>) {
    // `with_provider(f)` = `<ThemeContext.Provider value={…}>` + `f()`
    theme.provide(Theme::Dark, || {
        toolbar(theme);
        sidebar(theme);

        // A nested provider shadows the outer one for its own subtree only.
        theme.provide(Theme::HighContrast, || panel(theme));
        panel(theme); // still dark
    });

    println!("provider unmounted → {}", theme.get().name());
}

fn toolbar(theme: &Context<Theme>) {
    let button = use_effect(
        {
            let theme = theme.clone();
            move || println!("  [toolbar] button reads {}", use_context(&theme).name())
        },
        (),
    );
    let _ = button;
    println!("  [toolbar] mounted");
}

fn sidebar(theme: &Context<Theme>) {
    // Context reads are tracked, so these re-run when the theme changes —
    // without any component hierarchy existing at all.
    let links = use_effect(
        {
            let theme = theme.clone();
            move || {
                println!(
                    "  [sidebar] links restyled for {}",
                    use_context(&theme).name()
                )
            }
        },
        (),
    );
    let _ = links;
    println!("  [sidebar] mounted");
}

fn panel(theme: &Context<Theme>) {
    let text = use_effect(
        {
            let theme = theme.clone();
            move || println!("  [panel] text painted for {}", use_context(&theme).name())
        },
        (),
    );
    let _ = text;
    println!("  [panel] mounted");
}
