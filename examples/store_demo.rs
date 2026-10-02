//! The Zustand-style store: state that lives outside the tree, read with
//! selectors, observed by both hooks and plain `subscribe` listeners.
//!
//! ```text
//! cargo run --example store_demo
//! ```

use rok_ui_hooks::*;

#[derive(Clone)]
struct Cart {
    items: Vec<(String, u32)>,
    coupon: Option<String>,
}

impl Cart {
    fn new() -> Self {
        Cart {
            items: Vec::new(),
            coupon: None,
        }
    }

    fn total(&self) -> u32 {
        self.items.iter().map(|(_, price)| price).sum()
    }
}

fn summary(cart: &Cart) -> String {
    if cart.items.is_empty() {
        return "empty".into();
    }
    let names: Vec<&str> = cart.items.iter().map(|(title, _)| title.as_str()).collect();
    format!("[{}]", names.join(", "))
}

fn main() {
    // `create(initial)` lives once, at module level in a real app.
    let cart = create_store(Cart::new());

    println!("— subscribe (works without any effects) —");
    let sub = cart.subscribe(|next: &Cart, prev: &Cart| {
        println!(
            "  cart changed: {} → {} (total {} → {})",
            summary(prev),
            summary(next),
            prev.total(),
            next.total()
        );
    });

    // ── `useStore(store, selector)` — a tracked selector read ────────────────
    println!("\n— use_store (tracked, reruns the effect) —");
    let badge = use_effect(
        {
            let cart = cart.clone();
            move || {
                println!(
                    "  badge shows {} item(s)",
                    use_store(&cart, |c| c.items.len())
                )
            }
        },
        (),
    );

    // ── `createSelector` + deps → Zustand's bail-out ────────────────────────
    println!("\n— select() + deps (only re-runs when the slice changes) —");
    let total = cart.select(|c: &Cart| c.total());
    let total_label = use_effect(
        {
            let (cart, total) = (cart.clone(), total.clone());
            move || {
                let coupon = use_store(&cart, |c| c.coupon.clone());
                println!(
                    "  total {} (coupon: {})",
                    total.get(),
                    coupon.unwrap_or_else(|| "none".into())
                )
            }
        },
        (total.clone(),),
    );

    println!("\n— mutations —");
    cart.update(|c| c.items.push(("signals".into(), 20)));
    cart.update(|c| c.items.push(("book".into(), 30)));
    println!("  added two items"); // total changed → the selector effect runs
    cart.update(|c| c.items[0].1 = 10); // total changes again → runs
    println!("  repriced the first item");

    println!("\n— a mutation the total row does not care about —");
    cart.update(|c| c.items.push(("flyer".into(), 0))); // free item
    println!("  a free item was added: the badge reran, the total row stayed put");

    println!("\n— a tracked read the deps gate deliberately ignores —");
    cart.update(|c| c.coupon = Some("SAVE10".into()));
    println!("  coupon applied: the total row was notified, but its `(total,)` dep didn't move");

    drop((badge, total_label));
    drop(sub);
    println!("\nunsubscribed: further writes are silent");
    cart.set(Cart::new());
}
