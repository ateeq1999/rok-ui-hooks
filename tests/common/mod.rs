//! Shared test helper: a log the effects append to.

use std::cell::RefCell;
use std::rc::Rc;

pub fn log() -> (Rc<RefCell<Vec<String>>>, impl Fn(String) + Clone) {
    let entries = Rc::new(RefCell::new(Vec::new()));
    let sink = entries.clone();
    (entries, move |line: String| sink.borrow_mut().push(line))
}
