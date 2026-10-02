//! Async data: [`use_resource`] turns a future into a signal of
//! [`ResourceState`], and [`spawn`] runs futures on the local executor.

use std::cell::{Cell, RefCell};
use std::future::Future;
use std::rc::Rc;

use crate::executor::{Task, spawn};
use crate::signal::{ReadSignal, use_state};

/// What a [`Resource`] knows right now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResourceState<T> {
    /// Nothing was asked for yet.
    Idle,
    /// A fetch is in flight.
    Loading,
    /// The last fetch succeeded.
    Ready(T),
    /// The last fetch failed.
    Failed(String),
}

impl<T> ResourceState<T> {
    /// `true` while a fetch is in flight.
    pub fn is_loading(&self) -> bool {
        matches!(self, ResourceState::Loading)
    }

    /// `true` before the first fetch.
    pub fn is_idle(&self) -> bool {
        matches!(self, ResourceState::Idle)
    }

    /// The value, if the last fetch succeeded.
    pub fn value(&self) -> Option<&T> {
        match self {
            ResourceState::Ready(value) => Some(value),
            _ => None,
        }
    }

    /// The failure message, if the last fetch failed.
    pub fn error(&self) -> Option<&str> {
        match self {
            ResourceState::Failed(message) => Some(message),
            _ => None,
        }
    }

    /// Transform the value, leaving the state shape alone.
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> ResourceState<U> {
        match self {
            ResourceState::Idle => ResourceState::Idle,
            ResourceState::Loading => ResourceState::Loading,
            ResourceState::Ready(value) => ResourceState::Ready(f(value)),
            ResourceState::Failed(message) => ResourceState::Failed(message),
        }
    }
}

struct Inner {
    live: Rc<Cell<bool>>,
    task: Rc<RefCell<Option<Task>>>,
    refetch: RefCell<Box<dyn Fn()>>,
}

/// An asynchronous value with request state, cancellation and latest-wins
/// semantics.
///
/// Dropping the resource cancels the in-flight fetch, so a fetch that resolves
/// after its resource is gone is ignored.
///
/// The future is polled by [`crate::tick`], and by every wake it triggers.
pub struct Resource<T> {
    read: ReadSignal<ResourceState<T>>,
    inner: Rc<Inner>,
}

impl<T: Clone> Clone for Resource<T> {
    fn clone(&self) -> Self {
        Self {
            read: self.read.clone(),
            inner: self.inner.clone(),
        }
    }
}

impl<T: 'static> std::fmt::Debug for Resource<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = self.read.with_untracked(|state| match state {
            ResourceState::Idle => "idle",
            ResourceState::Loading => "loading",
            ResourceState::Ready(_) => "ready",
            ResourceState::Failed(_) => "failed",
        });
        f.debug_struct("Resource").field("state", &state).finish()
    }
}

impl<T: Clone + 'static> Resource<T> {
    /// The tracked state signal — read it inside an effect to react.
    pub fn state(&self) -> ReadSignal<ResourceState<T>> {
        self.read.clone()
    }

    /// A tracked read of the whole state.
    pub fn get(&self) -> ResourceState<T> {
        self.read.get()
    }

    /// An untracked peek, for assertions and logs.
    pub fn peek(&self) -> ResourceState<T> {
        self.read.get_untracked()
    }

    /// Throw the current value away and fetch again.
    pub fn refetch(&self) {
        (self.inner.refetch.borrow())();
    }
}

impl<T> Drop for Resource<T> {
    fn drop(&mut self) {
        if Rc::strong_count(&self.inner) == 1 {
            self.inner.live.set(false);
            let task = self.inner.task.borrow_mut().take();
            drop(task);
        }
    }
}

/// `createResource(fetcher)` → a [`Resource`] that starts fetching immediately.
///
/// ```
/// use rok_ui_hooks::*;
/// use std::future::Future;
/// use std::pin::Pin;
/// use std::task::{Context, Poll};
///
/// // A future that is ready on the second poll, to show the Loading state.
/// struct Later(u8);
/// impl Future for Later {
///     type Output = &'static str;
///     fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
///         if self.0 == 0 { return Poll::Pending }
///         Poll::Ready("done")
///     }
/// }
///
/// let user = use_resource(|| Later(1));
/// assert!(user.get().is_loading());
/// tick();
/// assert_eq!(user.get(), ResourceState::Ready("done"));
/// ```
pub fn use_resource<T, F, Fut>(fetcher: F) -> Resource<T>
where
    T: 'static,
    F: FnMut() -> Fut + 'static,
    Fut: Future<Output = T> + 'static,
{
    let (state, set_state) = use_state(ResourceState::<T>::Loading);
    let generation = Rc::new(Cell::new(0u64));
    let live = Rc::new(Cell::new(true));
    let task: Rc<RefCell<Option<Task>>> = Rc::new(RefCell::new(None));
    let fetcher = Rc::new(RefCell::new(fetcher));

    // Erased so `Resource<T>` does not need the fetcher's types.
    let fire = {
        let (fetcher, task, generation, live, set_state) = (
            fetcher.clone(),
            task.clone(),
            generation.clone(),
            live.clone(),
            set_state.clone(),
        );
        move || {
            let stamp = generation.get() + 1;
            generation.set(stamp);
            set_state.set(ResourceState::Loading);

            let future = (fetcher.borrow_mut())();
            let (generation, live, sink) = (generation.clone(), live.clone(), set_state.clone());
            let spawned = spawn(async move {
                let value = future.await;
                // Latest wins: a slow fetch must not overwrite a newer one, and
                // a disposed resource must not write at all.
                if !live.get() || generation.get() != stamp {
                    return;
                }
                sink.set(ResourceState::Ready(value));
            });
            *task.borrow_mut() = Some(spawned);
        }
    };

    let inner = Rc::new(Inner {
        live,
        task,
        refetch: RefCell::new(Box::new(fire)),
    });
    (inner.refetch.borrow())();

    Resource { read: state, inner }
}

/// `createResource` that maps a future's output through `select`, so only the
/// projected value lands in the resource.
///
/// ```
/// use rok_ui_hooks::*;
/// use std::future::Future;
/// use std::pin::Pin;
/// use std::task::{Context, Poll};
///
/// struct Ready(u8);
/// impl Future for Ready {
///     type Output = (u8, &'static str);
///     fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
///         Poll::Ready((self.0, "ok"))
///     }
/// }
///
/// let user_id = use_resource_with(|| Ready(7), |(id, _)| id);
/// tick();
/// assert_eq!(user_id.get(), ResourceState::Ready(7));
/// ```
pub fn use_resource_with<T, U, F, Fut>(
    mut fetcher: F,
    select: impl Fn(T) -> U + 'static,
) -> Resource<U>
where
    T: 'static,
    U: 'static,
    F: FnMut() -> Fut + 'static,
    Fut: Future<Output = T> + 'static,
{
    let select = Rc::new(select);
    use_resource(move || {
        let future = fetcher();
        let select = select.clone();
        async move { select(future.await) }
    })
}
