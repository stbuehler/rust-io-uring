use std::cell::RefCell;
use std::fmt;
use std::io;
use std::time::Duration;

use futures::Future;
use tokio_current_thread::{self, CurrentThread};
use tokio_executor::{self, Enter};
use tokio_uring_reactor::{self, Handle, Reactor};
use tokio_timer::{self, Timer};


pub struct Runtime {
	executor: CurrentThread<Timer<Reactor>>,
}

impl Runtime {
	/// Create new Runtime
	pub fn new() -> io::Result<Self> {
		let reactor = Reactor::new()?;
		let executor = CurrentThread::new_with_park(Timer::new(reactor));

		Ok(Runtime {
			executor,
		})
	}

	/// Spawn the future on the executor.
	///
	/// This internally queues the future to be executed once `run` is called.
	pub fn spawn<F>(&mut self, future: F) -> &mut Self
	where
		F: Future<Item = (), Error = ()> + 'static,
	{
		self.executor.spawn(future);
		self
	}

	/// Run the executor to completion, blocking the thread until **all**
	/// spawned futures have completed.
	pub fn run(&mut self) -> Result<(), tokio_current_thread::RunError> {
		let mut enter = tokio_executor::enter().unwrap();
		self.enter(&mut enter).run()
	}

	/// Run the executor to completion, blocking the thread until all
	/// spawned futures have completed **or** `duration` time has elapsed.
	pub fn run_timeout(
		&mut self,
		duration: Duration,
	) -> Result<(), tokio_current_thread::RunTimeoutError> {
		let mut enter = tokio_executor::enter().unwrap();
		self.enter(&mut enter).run_timeout(duration)
	}

	/// Synchronously waits for the provided `future` to complete.
	///
	/// Also waits for all other tasks to complete.
	///
	/// The outer `Result` represents possible event loop errors; on success it
	/// will return the `Future`s result (which can have a different error).
	pub fn block_on_all<F>(
		&mut self,
		future: F,
	) -> Result<F::Item, tokio_current_thread::BlockError<F::Error>>
	where
		F: Future,
	{
		let mut enter = tokio_executor::enter().unwrap();
		self.enter(&mut enter).block_on_all(future)
	}

	/// Perform a single iteration of the event loop.
	///
	/// This function blocks the current thread even if the executor is idle.
	pub fn turn(
		&mut self,
		duration: Option<Duration>,
	) -> Result<tokio_current_thread::Turn, tokio_current_thread::TurnError> {
		let mut enter = tokio_executor::enter().unwrap();
		self.enter(&mut enter).turn(duration)
	}

	/// Returns `true` if the executor is currently idle.
	///
	/// An idle executor is defined by not currently having any spawned tasks.
	///
	/// Timers / IO-watchers that are not associated with a spawned task are
	/// ignored.
	pub fn is_idle(&self) -> bool {
		self.executor.is_idle()
	}

	/// Bind `Runtime` instance with an execution context.
	pub fn enter<'a>(&'a mut self, enter: &'a mut Enter) -> Entered<'a> {
		Entered {
			runtime: self,
			enter,
		}
	}

	/// Get `Reactor` handle for this `Runtime`
	pub fn reactor_handle(&mut self) -> Handle {
		self.executor.get_park().get_park().handle()
	}

	/// Get `Timer` handle for this `Runtime`
	pub fn timer_handle(&mut self) -> tokio_timer::timer::Handle {
		self.executor.get_park().handle()
	}
}

impl fmt::Debug for Runtime {
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		self.executor.get_park().fmt(f)
	}
}

// Handle::current doesn't offer a "failable" access, it always spawns a new
// reactor if it can't find one
thread_local!(static CURRENT_REACTOR: RefCell<Option<Handle>> = RefCell::new(None));

fn with_reactor_handle<F, R>(handle: &Handle, enter: &mut Enter, f: F) -> R
where
	F: FnOnce(&mut Enter) -> R,
{
	struct Reset;

	impl Drop for Reset {
		fn drop(&mut self) {
			CURRENT_REACTOR.with(|current| {
				let mut current = current.borrow_mut();
				*current = None;
			});
		}
	}

	// make sure to always reset CURRENT_REACTOR
	let reset = Reset;

	CURRENT_REACTOR.with(|current| {
		let mut current = current.borrow_mut();
		*current = Some(handle.clone());
	});

	// also use this handle for Handle::current()
	let result = tokio_uring_reactor::with_default(handle, enter, f);

	// Here we want to usually reset the CURRENT_REACTOR, and on unwinding it
	// will get dropped automatically too.
	drop(reset);

	result
}

/// Get a `Handle` to the `Reactor` of the currently running `Runtime`.
///
/// Doesn't create a background thread with a new `Reactor` like
/// `Handle::current` if there is no `Handle` available (i.e. when the current
/// thread doesn't have an active `Runtime`).
pub fn current_reactor_handle() -> Option<Handle> {
	CURRENT_REACTOR.with(|current| current.borrow().clone())
}

/// A `Runtime` instance bound to a supplied execution context.
pub struct Entered<'a> {
	runtime: &'a mut Runtime,
	enter: &'a mut Enter,
}

impl<'a> Entered<'a> {
	fn with<F, T>(&mut self, f: F) -> T
	where
		F: FnOnce(Borrow) -> T,
	{
		let (reactor_handle, timer_handle) = {
			let timer = self.runtime.executor.get_park();
			(
				timer.get_park().handle(),
				timer.handle(),
			)
		};
		let runtime = &mut self.runtime;
		with_reactor_handle(&reactor_handle, self.enter, |enter| {
			tokio_timer::with_default(&timer_handle, enter, |enter| {
				f(Borrow {
					executor: runtime.executor.enter(enter),
				})
			})
		})
	}

	/// Spawn the future on the executor.
	///
	/// This internally queues the future to be executed once `run` is called.
	pub fn spawn<F>(&mut self, future: F) -> &mut Self
	where
		F: Future<Item = (), Error = ()> + 'static,
	{
		self.runtime.executor.spawn(future);
		self
	}

	/// Run the executor to completion, blocking the thread until **all**
	/// spawned futures have completed.
	pub fn run(&mut self) -> Result<(), tokio_current_thread::RunError> {
		self.with(|mut borrow| {
			borrow
				.executor
				.run()
		})
	}

	/// Run the executor to completion, blocking the thread until all
	/// spawned futures have completed **or** `duration` time has elapsed.
	pub fn run_timeout(
		&mut self,
		duration: Duration,
	) -> Result<(), tokio_current_thread::RunTimeoutError> {
		self.with(|mut borrow| {
			borrow
				.executor
				.run_timeout(duration)
		})
	}

	/// Synchronously waits for the provided `future` to complete.
	///
	/// Also waits for all other tasks to complete.
	///
	/// The outer `Result` represents possible event loop errors; on success it
	/// will return the `Future`s result (which can have a different error).
	pub fn block_on_all<F>(
		&mut self,
		future: F,
	) -> Result<F::Item, tokio_current_thread::BlockError<F::Error>>
	where
		F: Future,
	{
		self.with(|mut borrow| {
			let ret = borrow.executor.block_on(future);
			borrow.executor.run().unwrap();
			ret
		})
	}

	/// Perform a single iteration of the event loop.
	///
	/// This function blocks the current thread even if the executor is idle.
	pub fn turn(
		&mut self,
		duration: Option<Duration>,
	) -> Result<tokio_current_thread::Turn, tokio_current_thread::TurnError> {
		self.with(|mut borrow| {
			borrow
				.executor
				.turn(duration)
		})
	}

	/// Returns `true` if the executor is currently idle.
	///
	/// An idle executor is defined by not currently having any spawned tasks.
	///
	/// Timers / IO-watchers that are not associated with a spawned task are
	/// ignored.
	pub fn is_idle(&self) -> bool {
		self.runtime.executor.is_idle()
	}
}

impl<'a> fmt::Debug for Entered<'a> {
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		self.runtime.executor.get_park().fmt(f)
	}
}

struct Borrow<'a> {
	executor: tokio_current_thread::Entered<'a, Timer<Reactor>>,
}
