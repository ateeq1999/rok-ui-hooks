//! The Context API: defaults, providers, reactivity and scope capture.

mod common;

use common::log;
use rok_ui_hooks::*;

#[test]
fn reads_default_without_a_provider() {
    let ctx = create_context(10);
    assert_eq!(use_context(&ctx), 10);
    assert_eq!(ctx.get(), 10);
}

#[test]
fn provider_overrides_default_inside_the_scope() {
    let ctx = create_context(10);
    ctx.provide(20, || assert_eq!(use_context(&ctx), 20));
    assert_eq!(use_context(&ctx), 10);
}

#[test]
fn nested_providers_shadow_and_restore() {
    let ctx = create_context("light".to_string());
    ctx.provide("dark".to_string(), || {
        assert_eq!(use_context(&ctx), "dark");
        ctx.provide("high-contrast".to_string(), || {
            assert_eq!(use_context(&ctx), "high-contrast");
        });
        assert_eq!(use_context(&ctx), "dark");
    });
    assert_eq!(use_context(&ctx), "light");
}

#[test]
fn consumers_rerun_when_the_provider_value_changes() {
    let (logs, push) = log();
    let ctx = create_context("light".to_string());
    ctx.provide("dark".to_string(), || {
        let _e = use_effect(
            {
                let (ctx, p) = (ctx.clone(), push.clone());
                move || p(use_context(&ctx))
            },
            (),
        );
        ctx.set("solarized".into());
        assert_eq!(*logs.borrow(), ["dark", "solarized"]);
    });
}

#[test]
fn context_updates_are_visible_to_tracked_reads() {
    let (logs, push) = log();
    let ctx = create_context(0);
    let _e = use_effect(
        {
            let (ctx, p) = (ctx.clone(), push.clone());
            move || p(format!("{}", use_context(&ctx)))
        },
        (),
    );
    assert_eq!(*logs.borrow(), ["0"]);
    ctx.set(1); // no provider active → writes the default cell
    assert_eq!(*logs.borrow(), ["0", "1"]);
}

#[test]
fn computations_keep_the_context_of_their_creation_site() {
    let (logs, push) = log();
    let ctx = create_context(0);
    let (tick, set_tick) = use_state(0);

    // The effect is created inside the provider scope but re-runs after it ended.
    let _e = ctx.provide(42, || {
        use_effect(
            {
                let (ctx, tick, p) = (ctx.clone(), tick.clone(), push.clone());
                move || p(format!("{}+{}", use_context(&ctx), tick.get()))
            },
            (),
        )
    });

    set_tick.set(1);
    // `use_context` resolves through the captured provider, not the empty stack.
    assert_eq!(*logs.borrow(), ["42+0", "42+1"]);
    // Outside the scope a plain read still sees the default.
    assert_eq!(use_context(&ctx), 0);
}

#[test]
fn a_context_can_be_set_from_outside_its_scope() {
    let ctx = create_context(1i32);
    let (seen, push) = log();
    let _e = use_effect(
        {
            let (ctx, p) = (ctx.clone(), push.clone());
            move || p(use_context(&ctx).to_string())
        },
        (),
    );
    ctx.set(2);
    ctx.set(3);
    assert_eq!(*seen.borrow(), ["1", "2", "3"]);
}
