use signals::*;

fn main() {
    println!("1. Create");
    let (first, _set_first) = create_signal("John".to_string());
    let (last, set_last) = create_signal("Smith".to_string());
    let (show_full, set_show_full) = create_signal(true);

    let display = create_memo(move || {
        println!("   ### executing display_name");
        on_cleanup(|| println!("   ### releasing display_name dependencies"));
        if !show_full.get() {
            return first.get();
        }
        format!("{} {}", first.get(), last.get())
    });

    let _effect = create_effect(move || println!("My name is {}", display.get()));

    println!("2. Set show_full: false");
    set_show_full.set(false);
    println!("3. Change last name");
    set_last.set("Legend".into());
    println!("4. Set show_full: true");
    set_show_full.set(true);
}
