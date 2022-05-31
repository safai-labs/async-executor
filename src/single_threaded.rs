//! # Single-threaded executor
//!
//! This executor works *strictly* in a single-threaded environment. In order to spawn a task, use
//! [`spawn`]. To run the executor, use [`run`].
//!
//! There is no need to create an instance of the executor, it's automatically provisioned as a
//! thread-local instance.
//!
//! ## Example
//!
//! ```
//! use tokio::sync::*;
//! use wasm_rs_async_executor::single_threaded::{spawn, run};
//! let (sender, receiver) = oneshot::channel::<()>();
//! let _task = spawn(async move {
//!    // Complete when something is received
//!    let _ = receiver.await;
//! });
//! // Send data to be received
//! let _ = sender.send(());
//! run(None);
//! ```
use futures::channel::oneshot;
use futures::task::{waker_ref, ArcWake};
#[cfg(feature = "debug")]
use std::any::{type_name, TypeId};
use std::cell::UnsafeCell;
use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

/// Task token
type Token = usize;

#[cfg(feature = "debug")]
#[derive(Clone, Debug)]
#[allow(missing_docs)]
pub struct TypeInfo {
    type_id: Option<TypeId>,
    type_name: &'static str,
}

#[cfg(feature = "debug")]
impl TypeInfo {
    fn new<T>() -> Self
    where
        T: 'static,
    {
        Self {
            type_name: type_name::<T>(),
            type_id: Some(TypeId::of::<T>()),
        }
    }

    fn new_non_static<T>() -> Self {
        Self {
            type_name: type_name::<T>(),
            type_id: None,
        }
    }

    /// Returns tasks's type name
    pub fn type_name(&self) -> &'static str {
        self.type_name
    }

    /// Returns tasks's [`std::any::TypeId`]
    ///
    /// If it's `None` then the type does not have a `'static` lifetime
    pub fn type_id(&self) -> Option<TypeId> {
        self.type_id
    }
}

/// Task information
#[derive(Clone)]
#[must_use]
pub struct Task {
    token: Token,
    #[cfg(feature = "debug")]
    type_info: Arc<TypeInfo>,
}

impl PartialEq for Task {
    fn eq(&self, other: &Self) -> bool {
        self.token == other.token
    }
}

impl Eq for Task {}

impl PartialOrd for Task {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        self.token.partial_cmp(&other.token)
    }
}

impl Ord for Task {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.token.cmp(&other.token)
    }
}

impl Task {
    #[cfg(feature = "debug")]
    #[allow(missing_docs)]
    pub fn type_info(&self) -> &TypeInfo {
        self.type_info.as_ref()
    }
}

/// Task handle
///
/// Implements [`std::future::Future`] to allow for waiting for task completion
pub struct TaskHandle<T> {
    receiver: oneshot::Receiver<T>,
    task: Task,
}

impl<T> TaskHandle<T> {
    /// Returns a copy of task information record
    pub fn task(&self) -> Task {
        self.task.clone()
    }
}

/// Task joining error
#[derive(Debug, Clone)]
pub enum JoinError {
    /// Task was canceled
    Canceled,
}

impl<T> Future for TaskHandle<T> {
    type Output = Result<T, JoinError>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.receiver.try_recv() {
            Err(oneshot::Canceled) => Poll::Ready(Err(JoinError::Canceled)),
            Ok(Some(result)) => Poll::Ready(Ok(result)),
            Ok(None) => {
                cx.waker().wake_by_ref();
                Poll::Pending
            }
        }
    }
}

impl ArcWake for Task {
    fn wake_by_ref(arc_self: &Arc<Self>) {
        EXECUTOR.with(|cell| (unsafe { &mut *cell.get() }).enqueue(arc_self.clone()));
    }
}

/// Single-threaded executor
struct Executor {
    counter: Token,
    futures: BTreeMap<Task, Pin<Box<dyn Future<Output = ()>>>>,
    queue: Vec<Arc<Task>>,
}

impl Executor {
    fn new() -> Self {
        Self {
            counter: 0,
            futures: BTreeMap::new(),
            queue: vec![],
        }
    }

    fn enqueue(&mut self, task: Arc<Task>) {
        if self.futures.contains_key(&task) {
            self.queue.insert(0, task);
        }
    }

    fn spawn<F, T>(&mut self, fut: F) -> TaskHandle<T>
    where
        F: Future<Output = T> + 'static,
        T: 'static,
    {
        let token = self.counter;
        self.counter = self.counter.wrapping_add(1);
        let task = Task {
            token,
            #[cfg(feature = "debug")]
            type_info: Arc::new(TypeInfo::new::<F>()),
        };

        let (sender, receiver) = oneshot::channel();

        self.futures.insert(task.clone(), unsafe {
            Pin::new_unchecked(Box::new(async move {
                let _ = sender.send(fut.await);
            }))
        });
        self.queue.push(Arc::new(task.clone()));
        TaskHandle { receiver, task }
    }

    fn spawn_non_static<F, T>(&mut self, fut: F) -> TaskHandle<T>
    where
        F: Future<Output = T>,
    {
        let token = self.counter;
        self.counter = self.counter.wrapping_add(1);
        let task = Task {
            token,
            #[cfg(feature = "debug")]
            type_info: Arc::new(TypeInfo::new_non_static::<F>()),
        };

        let (sender, receiver) = oneshot::channel();

        self.futures.insert(task.clone(), unsafe {
            Pin::new_unchecked(std::mem::transmute::<_, Box<dyn Future<Output = ()>>>(
                Box::new(async move {
                    let _ = sender.send(fut.await);
                }) as Box<dyn Future<Output = ()>>,
            ))
        });
        self.queue.push(Arc::new(task.clone()));
        TaskHandle { receiver, task }
    }
}

thread_local! {
  static EXECUTOR: UnsafeCell<Executor> = UnsafeCell::new(Executor::new()) ;
}

thread_local! {
  static UNTIL: UnsafeCell<Option<Task>> = UnsafeCell::new(None) ;
}

thread_local! {
  static UNTIL_SATISFIED: UnsafeCell<bool> = UnsafeCell::new(false) ;
}

thread_local! {
  static WHILE_FN: UnsafeCell<Option<Box<dyn FnMut() -> bool>>> = UnsafeCell::new(None) ;
}

thread_local! {
  static YIELD: UnsafeCell<bool> = UnsafeCell::new(true) ;
}

thread_local! {
  static EXIT_LOOP: UnsafeCell<bool> = UnsafeCell::new(false) ;
}

/// Spawn a task
pub fn spawn<F, T>(fut: F) -> TaskHandle<T>
where
    F: Future<Output = T> + 'static,
    T: 'static,
{
    EXECUTOR.with(|cell| (unsafe { &mut *cell.get() }).spawn(fut))
}

/// Run tasks until completion of a future
///
/// ## Important
///
/// This function will yield to the environment if configured to do so.
///
pub fn run<F, R>(fut: F) -> R
where
    F: Future<Output = R>,
{
    let mut handle = EXECUTOR.with(|cell| (unsafe { &mut *cell.get() }).spawn_non_static(fut));
    YIELD.with(|cell| unsafe {
        *cell.get() = false;
    });
    run_until(handle.task());
    YIELD.with(|cell| unsafe {
        *cell.get() = true;
    });
    loop {
        match handle.receiver.try_recv() {
            Ok(None) => {}
            Ok(Some(v)) => return v,
            Err(_) => unreachable!(), // the data was sent at this point
        }
    }
}

/// Run the executor
///
/// The `until` promise and `while` function will remain unchanged.
pub fn start() {
    run_internal();
}

/// Reset execution conditions
///
/// Unsets the until promise and the while fn as well as their resolution statuses.
pub fn reset_yield_conditions() {
    UNTIL.with(|cell| unsafe { *cell.get() = None });
    UNTIL_SATISFIED.with(|cell| unsafe { *cell.get() = false });
    WHILE_FN.with(|cell| unsafe { *cell.get() = None });
}

/// Run the executor until a promise resolves
///
/// If `until` is `None`, it will run until all tasks have been completed. Otherwise, it'll wait
/// until passed task is complete, or unless a `cooperative` feature has been enabled and control
/// has been yielded to the environment. In this case the function will return but the environment
/// might schedule further execution of this executor in the background after termination of the
/// function enclosing invocation of this [`run`]
pub fn run_until(until: Task) {
    UNTIL.with(|cell| unsafe { *cell.get() = Some(until) });
    UNTIL_SATISFIED.with(|cell| unsafe { *cell.get() = false });
    run_internal();
}

/// Run the executor while a function returns true
///
/// The function passed as `condition` will run on every loop of the executor. The executor will
/// yield anytime the `condition` evaluates to `true`. You can restart execution by issuing another
/// `run` command
pub fn run_while<F>(condition: F)
where
    F: FnMut() -> bool + 'static,
{
    WHILE_FN.with(|cell| unsafe { *cell.get() = Some(Box::new(condition)) });

    run_internal();
}

// Returns `true` if `until` task completed, or there was no `until` task and every task was
// completed.
//
// Returns `false` if loop exit was requested
fn run_internal() -> bool {
    let until = UNTIL.with(|cell| unsafe { &*cell.get() });
    let exit_condition_met = UNTIL_SATISFIED.with(|cell| unsafe { *cell.get() });
    if exit_condition_met {
        return true;
    }
    EXECUTOR.with(|cell| loop {
        let task = (unsafe { &mut *cell.get() }).queue.pop();

        if let Some(task) = task {
            let future = (unsafe { &mut *cell.get() }).futures.get_mut(&task);
            let ready = future.map_or(false, |future| {
                let waker = waker_ref(&task);
                let context = &mut Context::from_waker(&*waker);
                let ready = matches!(future.as_mut().poll(context), Poll::Ready(_));
                ready
            });
            if ready {
                (unsafe { &mut *cell.get() }).futures.remove(&task);

                if let Some(Task { ref token, .. }) = until {
                    if *token == task.token {
                        UNTIL_SATISFIED.with(|cell| unsafe { *cell.get() = true });
                        return true;
                    }
                }
            }
        }
        if until.is_none() && (unsafe { &mut *cell.get() }).futures.is_empty() {
            UNTIL_SATISFIED.with(|cell| unsafe { *cell.get() = true });
            return true;
        }

        let should_continue =
            WHILE_FN.with(|cell| unsafe { (&mut *cell.get()).as_mut().map_or(true, |f| (f)()) });

        let exit_requested = EXIT_LOOP.with(|cell| {
            let v = cell.get();
            let result = unsafe { *v };
            // Clear the flag
            unsafe {
                *v = false;
            }
            result
        }) && YIELD.with(|cell| unsafe { *cell.get() });

        if exit_requested || !should_continue {
            return false;
        }

        if (unsafe { &mut *cell.get() }).queue.is_empty()
            && !(unsafe { &mut *cell.get() }).futures.is_empty()
        {
            // the executor is starving
            for task in (unsafe { &mut *cell.get() }).futures.keys() {
                (unsafe { &mut *cell.get() }).enqueue(Arc::new(task.clone()));
            }
        }
    })
}

/// Returns the number of tasks currently registered with the executor
#[must_use]
pub fn tasks_count() -> usize {
    EXECUTOR.with(|cell| {
        let executor = unsafe { &mut *cell.get() };
        executor.futures.len()
    })
}

/// Returns the number of tasks currently in the queue to execute
#[must_use]
pub fn queued_tasks_count() -> usize {
    EXECUTOR.with(|cell| (unsafe { &mut *cell.get() }).queue.len())
}

/// Returns all tasks that haven't completed yet
#[must_use]
pub fn tasks() -> Vec<Task> {
    EXECUTOR.with(|cell| {
        (unsafe { &*cell.get() })
            .futures
            .keys()
            .map(Task::clone)
            .collect()
    })
}

/// Returns tokens for queued tasks
#[must_use]
pub fn queued_tasks() -> Vec<Task> {
    EXECUTOR.with(|cell| {
        (unsafe { &*cell.get() })
            .queue
            .iter()
            .map(|t| Task::clone(t))
            .collect()
    })
}

/// Removes all tasks from the executor
///
/// ## Caution
///
/// Evicted tasks won't be able to get re-scheduled when they will be woken up.
pub fn evict_all() {
    EXECUTOR.with(|cell| unsafe { *cell.get() = Executor::new() });
}

#[cfg(test)]
fn set_counter(counter: usize) {
    EXECUTOR.with(|cell| (unsafe { &mut *cell.get() }).counter = counter);
}

#[cfg(test)]
mod tests {

    use super::*;
    thread_local! {
      static NUM: UnsafeCell<u32> = UnsafeCell::new(0) ;
    }

    #[test]
    fn test() {
        use tokio::sync::*;
        let (sender, receiver) = oneshot::channel::<()>();
        let _handle = spawn(async move {
            let _ = receiver.await;
        });
        let _ = sender.send(());
        start();
        reset_yield_conditions();
        evict_all();
    }

    #[test]
    fn test_until() {
        use tokio::sync::*;
        let (_sender1, receiver1) = oneshot::channel::<()>();
        let _handle1 = spawn(async move {
            let _ = receiver1.await;
        });
        let (sender2, receiver2) = oneshot::channel::<()>();
        let handle2 = spawn(async move {
            let _ = receiver2.await;
        });
        let _ = sender2.send(());
        run_until(handle2.task());
        reset_yield_conditions();
        evict_all();
    }

    #[test]
    fn test_while() {
        use tokio::sync::*;
        let (_sender1, receiver1) = oneshot::channel::<()>();
        let _handle1 = spawn(async move {
            let _ = receiver1.await;
        });
        let (sender2, receiver2) = oneshot::channel::<()>();
        let _handle2 = spawn(async move {
            let _ = receiver2.await;
        });
        let _ = sender2.send(());

        run_while(move || {
            let num = NUM.with(|cell| unsafe {
                *cell.get() += 1;
                *cell.get()
            });
            num < 6
        });
        let num = NUM.with(|cell| unsafe { *cell.get() });

        assert_eq!(num, 6);

        reset_yield_conditions();

        evict_all();
    }

    #[test]
    fn test_counts() {
        use tokio::sync::oneshot;
        let (sender, mut receiver) = oneshot::channel();
        let (sender2, receiver2) = oneshot::channel::<()>();
        let handle1 = spawn(async move {
            let _ = receiver2.await;
            let _ = sender.send((tasks_count(), queued_tasks_count()));
        });
        let _handle2 = spawn(async move {
            let _ = sender2.send(());
            futures::future::pending::<()>().await; // this will never end
        });
        run_until(handle1.task());
        let (tasks_, queued_tasks_) = receiver.try_recv().unwrap();
        // handle1 + handle2
        assert_eq!(tasks_, 2);
        // handle1 is being executed, handle2 has nothing new
        assert_eq!(queued_tasks_, 0);
        // handle1 is gone
        assert_eq!(tasks_count(), 1);
        // handle2 still has nothing new
        assert_eq!(queued_tasks_count(), 0);
        reset_yield_conditions();
        evict_all();
    }

    #[test]
    fn evicted_tasks_dont_requeue() {
        use tokio::sync::*;
        let (_sender, receiver) = oneshot::channel::<()>();
        let handle = spawn(async move {
            let _ = receiver.await;
        });
        assert_eq!(tasks_count(), 1);
        evict_all();
        assert_eq!(tasks_count(), 0);
        ArcWake::wake_by_ref(&Arc::new(handle.task()));
        assert_eq!(tasks_count(), 0);
        assert_eq!(queued_tasks_count(), 0);
        reset_yield_conditions();
        evict_all();
    }

    #[test]
    fn token_exhaustion() {
        set_counter(usize::MAX);
        // this should be fine anyway
        let handle0 = spawn(async move {});
        // this should NOT crash
        let handle = spawn(async move {});
        // new token should be different and wrap back to the beginning
        assert!(handle.task().token != handle0.task().token);
        assert_eq!(handle.task().token, 0);
        reset_yield_conditions();
        evict_all();
    }

    #[test]
    fn blocking_on() {
        use tokio::sync::*;
        let (sender, receiver) = oneshot::channel::<u8>();
        let _handle = spawn(async move {
            let _ = sender.send(1);
        });
        let result = run(async move { receiver.await.unwrap() });
        assert_eq!(result, 1);
        reset_yield_conditions();
        evict_all();
    }

    #[test]
    fn starvation() {
        use tokio::sync::*;
        let (sender, receiver) = oneshot::channel();
        let _handle = spawn(async move {
            tokio::task::yield_now().await;
            tokio::task::yield_now().await;
            let _ = sender.send(());
        });
        run(async move { receiver.await.unwrap() });
        reset_yield_conditions();
        evict_all();
    }

    #[cfg(feature = "debug")]
    #[test]
    fn task_type_info() {
        spawn(futures::future::pending::<()>());
        assert!(tasks()[0]
            .type_info()
            .type_name()
            .contains("future::pending::Pending"));
        assert_eq!(
            tasks()[0].type_info().type_id().unwrap(),
            TypeId::of::<futures::future::Pending<()>>()
        );
        reset_yield_conditions();
        evict_all();
        assert_eq!(tasks().len(), 0);
    }

    #[test]
    fn joining() {
        use tokio::sync::*;
        let (sender, receiver) = oneshot::channel();
        let (sender1, mut receiver1) = oneshot::channel();
        let _handle1 = spawn(async move {
            let _ = sender.send(());
        });

        let handle2 = spawn(async move {
            let _ = receiver.await;
            100u8
        });

        let handle3 = spawn(async move {
            let _ = sender1.send(handle2.await);
        });
        run_until(handle3.task());

        assert_eq!(receiver1.try_recv().unwrap().unwrap(), 100);
        reset_yield_conditions();

        evict_all();
    }
}
