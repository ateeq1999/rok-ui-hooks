//! Keyed lists: reconciliation by identity instead of by position.
//!
//! A `<For>`-style list is the reason a reactivity engine needs ownership. Each
//! key owns its own scope, so a row's effects, memos and resources live exactly
//! as long as that row does — reordering the list does not tear them down,
//! removing a row does.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use crate::runtime::{Node, run_in_owner};

struct Entry<T, N> {
    item: T,
    output: N,
    owner: Rc<Node>,
}

type Render<T, N> = RefCell<dyn FnMut(&T) -> N>;

/// A live list of keyed rows, each with its own owned scope.
///
/// A key that survives a reconcile keeps its scope, and therefore its state. A row
/// whose item changed is re-rendered in place; a row whose key disappeared is
/// disposed, running its cleanups.
pub struct KeyedList<T, K, N> {
    entries: RefCell<BTreeMap<K, Entry<T, N>>>,
    /// Keys in list order, which is not key order.
    order: RefCell<Vec<K>>,
    root: Rc<Node>,
    key: Rc<dyn Fn(&T) -> K>,
    render: Rc<Render<T, N>>,
}

impl<T, K, N> std::fmt::Debug for KeyedList<T, K, N> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeyedList")
            .field("len", &self.entries.borrow().len())
            .finish()
    }
}

impl<T: PartialEq + Clone + 'static, K: Ord + Clone + 'static, N: 'static> KeyedList<T, K, N> {
    /// Diff `items` against the live rows: add what is new, re-render what
    /// changed, dispose what is gone.
    ///
    /// New rows adopt the list's own scope, so dropping the list disposes them.
    pub fn reconcile(&self, items: impl IntoIterator<Item = T>) {
        let items: Vec<T> = items.into_iter().collect();
        run_in_owner(self.root.id, || self.reconcile_inner(items));
    }

    fn reconcile_inner(&self, items: Vec<T>) {
        let wanted: Vec<K> = items.iter().map(|item| (self.key)(item)).collect();
        let positions: BTreeMap<K, usize> = wanted
            .iter()
            .cloned()
            .enumerate()
            .map(|(index, key)| (key, index))
            .collect();

        let mut entries = self.entries.borrow_mut();

        // Dispose rows whose key is gone. The list's own scope holds every row
        // alive, so dropping the entry is not enough — it has to be disposed.
        let stale: Vec<K> = entries
            .keys()
            .filter(|key| !positions.contains_key(*key))
            .cloned()
            .collect();
        for key in stale {
            if let Some(entry) = entries.remove(&key) {
                crate::runtime::dispose_node(entry.owner.id);
            }
        }

        for (item, key) in items.into_iter().zip(wanted) {
            match entries.get_mut(&key) {
                Some(entry) => {
                    if entry.item != item {
                        entry.item = item;
                        entry.output = self.render_entry(&entry.owner, &entry.item);
                    }
                }
                None => {
                    let owner = Node::create_owner();
                    let output = self.render_entry(&owner, &item);
                    entries.insert(
                        key,
                        Entry {
                            item,
                            output,
                            owner,
                        },
                    );
                }
            }
        }

        let mut order = self.order.borrow_mut();
        order.clear();
        order.extend(entries.keys().cloned());
        order.sort_by_key(|key| positions.get(key).copied().unwrap_or(usize::MAX));
    }

    fn render_entry(&self, owner: &Rc<Node>, item: &T) -> N {
        // Drop whatever the previous render owned before running the new one.
        owner.reset();
        run_in_owner(owner.id, || (self.render.borrow_mut())(item))
    }

    /// The row outputs, in list order.
    pub fn entries(&self) -> Vec<N>
    where
        N: Clone,
    {
        let entries = self.entries.borrow();
        self.order
            .borrow()
            .iter()
            .filter_map(|key| entries.get(key).map(|entry| entry.output.clone()))
            .collect()
    }

    /// The output of one row.
    pub fn get(&self, key: &K) -> Option<N>
    where
        N: Clone,
    {
        self.entries
            .borrow()
            .get(key)
            .map(|entry| entry.output.clone())
    }

    /// How many rows are alive.
    pub fn len(&self) -> usize {
        self.entries.borrow().len()
    }

    /// `true` when the list has no rows.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The keys currently alive, in list order.
    pub fn keys(&self) -> Vec<K> {
        self.order.borrow().clone()
    }
}

/// Build a keyed list. Rows render once; use [`KeyedList::reconcile`] to diff.
///
/// ```
/// use rok_ui_hooks::*;
///
/// let list = create_keyed_list(vec![1, 2, 3], |item: &i32| *item, |item: &i32| {
///     println!("row {item}");
///     format!("#{item}")
/// });
/// assert_eq!(list.keys(), [1, 2, 3]);
///
/// list.reconcile(vec![3, 1]); // 2 is gone, 1 and 3 keep their scope
/// assert_eq!(list.keys(), [3, 1]);
///
/// list.reconcile(vec![3, 1, 9]); // 9 is new
/// assert_eq!(list.entries(), ["#3", "#1", "#9"]);
/// ```
pub fn create_keyed_list<T, K, N>(
    items: impl IntoIterator<Item = T>,
    key: impl Fn(&T) -> K + 'static,
    render: impl FnMut(&T) -> N + 'static,
) -> KeyedList<T, K, N>
where
    T: PartialEq + Clone + 'static,
    K: Ord + Clone + 'static,
    N: 'static,
{
    let list = KeyedList {
        entries: RefCell::new(BTreeMap::new()),
        order: RefCell::new(Vec::new()),
        root: Node::create_owner(),
        key: Rc::new(key),
        render: Rc::new(RefCell::new(render)),
    };
    list.reconcile(items);
    list
}
