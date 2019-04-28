mod async_poll;
mod async_read;
mod async_write;

use std::{
	cell::UnsafeCell,
	convert::Infallible,
	fmt,
	io,
	os::unix::io::{RawFd, AsRawFd},
	pin::Pin,
	rc::{Rc, Weak},
	time::Duration,
};

use crate::{
	registration::{
		RawRegistration,
		UringResult,
	},
	unpark,
};

pub use self::async_poll::AsyncPoll;
pub use self::async_read::AsyncRead;
pub use self::async_write::AsyncWrite;

fn iovec_from(data: &[u8]) -> libc::iovec {
	libc::iovec {
		iov_base: data.as_ptr() as *mut libc::c_void,
		iov_len: data.len(),
	}
}

fn iovec_empty() -> libc::iovec {
	libc::iovec {
		iov_base: 0 as *mut libc::c_void,
		iov_len: 0,
	}
}

fn sq_full_map_err(_error: io_uring::SubmissionError<Infallible>) -> io::Error {
	io::Error::new(io::ErrorKind::Other, "submission queue full")
}

pub struct Unpark(unpark::Unpark);

impl tokio_executor::park::Unpark for Unpark {
	fn unpark(&self) {
		self.0.unpark();
	}
}

// stuff we need to mutate during uring completion handling
struct CompletionState {
	requeue_timer: bool,
	timer_pending: bool,
	requeue_park: bool,
	active_wait: usize,
	park: unpark::Park,
}

impl CompletionState {
	// special events must be "odd"
	const TIMER: u64 = 0x1;
	const PARK: u64 = 0x3;

	fn new() -> io::Result<Self> {
		Ok(CompletionState {
			requeue_timer: true,
			timer_pending: false,
			requeue_park: true,
			active_wait: 0,
			park: unpark::Park::new()?,
		})
	}

	fn handle_completion(&mut self, user_data: u64, result: UringResult) {
		if 0 == user_data {
			// fire-and-forget command (POLL_DEL)
			return;
		}
		self.active_wait -= 1;
		if 0 == user_data & 0x1 {
			let mut reg = unsafe { RawRegistration::from_user_data(user_data) };
			reg.notify(result);
		} else {
			match user_data {
				CompletionState::TIMER => {
					// wakeup by timer, just requeue read and rearm/disable next turn
					self.requeue_timer = true;
					self.timer_pending = true;
				},
				CompletionState::PARK => {
					// wakeup by park, just requeue read
					self.park.clear_event();
					self.requeue_park = true;
				},
				_ => panic!("unknown event: {}", user_data),
			}
		}
	}
}

struct Inner {
	// FIXME: on shutdown need to clear (wait for completion!) *at
	// least* internal operations before freeing memory
	uring: io_uring::Uring,
	completion_state: CompletionState,
	timerfd: timerfd::TimerFd,
	read_buf: [u8; 32], // for various wakeup mechanisms
	read_iovec: [libc::iovec; 1],
}

impl Inner {
	fn build() -> io::Result<Self> {
		let params = io_uring::SetupParameters::new(io_uring::SetupFlags::default());

		Ok(Inner {
			uring: io_uring::Uring::new(4096, params)?,
			completion_state: CompletionState::new()?,
			timerfd: timerfd::TimerFd::new()?,
			read_buf: [0u8; 32],
			read_iovec: [ iovec_empty() ],
		})
	}

	fn init(mut self: Pin<&mut Self>) {
		let iovec = iovec_from(&self.as_ref().read_buf);
		self.as_mut().read_iovec[0] = iovec;
	}

	// returns true if at least one completion was received
	fn check_completions(&mut self) -> bool {
		let mut received_completion: bool = false;

		for cqe in self.uring.completion_queue().into_iter() {
			received_completion = true;

			let result = UringResult {
				result: cqe.res,
				flags: cqe.flags,
			};

			self.completion_state.handle_completion(cqe.user_data, result);
		}

		received_completion
	}

	fn park_inner(&mut self, mut wait: bool, timeout: Option<Duration>) -> io::Result<()> {
		if self.check_completions() {
			// don't wait for new events below; we first need to handle this one
			wait = false;
		}

		// proper check later, but don't need to setup various things if
		// we already know we're not going to wait
		if self.completion_state.park.pending() {
			wait = false;
		}

		if wait {
			// set timer before we requeue it
			if let Some(timeout) = timeout {
				log::trace!("wait with timeout: {:?}", timeout);
				debug_assert!(timeout != Duration::new(0, 0)); // "zero" timer must trigger wait = false
				self.timerfd.set_state(timerfd::TimerState::Oneshot(timeout), timerfd::SetTimeFlags::Default);
				self.completion_state.timer_pending = false;
			} else {
				log::trace!("wait without timeout");
				if self.completion_state.timer_pending {
					// disarm timer after it triggered (instead of reading it)
					self.timerfd.set_state(timerfd::TimerState::Disarmed, timerfd::SetTimeFlags::Default);
					self.completion_state.timer_pending = false;
				}
			}

			if self.completion_state.requeue_timer {
				if self.queue_timer_poll().is_err() {
					// never wait if submission queue is full and we couldn't insert timer
					wait = false;
				} else {
					self.completion_state.requeue_timer = false;
				}
			}
		}

		if wait && self.completion_state.requeue_park {
			if self.queue_park_read().is_err() {
				// never wait if submission queue is full and we couldn't insert park
				wait = false;
			} else {
				self.completion_state.requeue_park = false;
			}
		}

		let pending = self.uring.submission_queue().pending_submissions();

		{
			let park_enter = self.completion_state.park.enter();
			if !park_enter.allow_wait {
				wait = false;
			}

			if 0 == pending && !wait {
				log::trace!("nothing to submit and not waiting, not calling io_uring_enter");
				return Ok(());
			}

			let (min_complete, flags) = if wait {
				(1, io_uring::EnterFlags::GETEVENTS)
			} else {
				// submit only
				(0, io_uring::EnterFlags::default())
			};

			log::trace!(
				"io_uring_enter: (to_submit = {}, min_complete = {}, flags = {:?}, sig = None)",
				pending,
				min_complete,
				flags,
			);
			self.uring.file().enter(pending, min_complete, flags, None)?;
		}

		self.check_completions();

		Ok(())
	}

	fn park(&mut self) -> io::Result<()> {
		self.park_inner(true, None)
	}

	fn park_timeout(&mut self, duration: Duration) -> io::Result<()> {
		if duration == Duration::new(0, 0) {
			// don't wait at all
			self.park_inner(false, None)
		} else {
			self.park_inner(true, Some(duration))
		}
	}

	fn queue_timer_poll(&mut self) -> Result<(), io_uring::SubmissionError<Infallible>> {
		let fd = self.timerfd.as_raw_fd();
		self.uring.submission_queue().bulk().submit_with(|entry| {
			entry.poll_add(
				io_uring::FileDescriptor::FD(fd),
				io_uring::PollFlags::IN,
			);
			entry.user_data = CompletionState::TIMER;
			Ok(())
		})?;
		self.completion_state.active_wait += 1;
		Ok(())
	}

	fn queue_park_read(&mut self) -> Result<(), io_uring::SubmissionError<Infallible>> {
		let fd = self.completion_state.park.as_raw_fd();
		//let iovec = &self.read_iovec;
		self.uring.submission_queue().bulk().submit_with(|entry| {
			entry.poll_add(
				io_uring::FileDescriptor::FD(fd),
				io_uring::PollFlags::IN,
			);

/*
			unsafe {
				entry.readv(
					io_uring::IoPriority::None,
					io_uring::FileDescriptor::FD(fd),
					0,
					io_uring::ReadWriteFlags::default(),
					iovec,
				);
			}
*/

			entry.user_data = CompletionState::PARK;
			Ok(())
		})?;
		self.completion_state.active_wait += 1;
		Ok(())
	}

	fn queue_async_read(&mut self, fd: RawFd, offset: u64, iovec: *const [libc::iovec], reg: RawRegistration) -> io::Result<()> {
		self.uring.submission_queue().bulk().submit_with(|entry| {
			unsafe {
				entry.readv(
					io_uring::IoPriority::None,
					io_uring::FileDescriptor::FD(fd),
					offset,
					io_uring::ReadWriteFlags::default(),
					iovec,
				);
				entry.user_data = reg.into_user_data();
			}
			Ok(())
		}).map_err(sq_full_map_err)?;
		self.completion_state.active_wait += 1;
		Ok(())
	}

	fn queue_async_write(&mut self, fd: RawFd, offset: u64, iovec: *const [libc::iovec], reg: RawRegistration) -> io::Result<()> {
		self.uring.submission_queue().bulk().submit_with(|entry| {
			unsafe {
				entry.writev(
					io_uring::IoPriority::None,
					io_uring::FileDescriptor::FD(fd),
					offset,
					io_uring::ReadWriteFlags::default(),
					iovec,
				);
				entry.user_data = reg.into_user_data();
			}
			Ok(())
		}).map_err(sq_full_map_err)?;
		self.completion_state.active_wait += 1;
		Ok(())
	}

	fn queue_async_poll(&mut self, fd: RawFd, flags: io_uring::PollFlags, reg: RawRegistration) -> io::Result<()> {
		self.uring.submission_queue().bulk().submit_with(|entry| {
			unsafe {
				entry.poll_add(
					io_uring::FileDescriptor::FD(fd),
					flags,
				);
				entry.user_data = reg.into_user_data();
			}
			Ok(())
		}).map_err(sq_full_map_err)?;
		self.completion_state.active_wait += 1;
		Ok(())
	}
}

struct InnerMut {
	inner: Rc<UnsafeCell<Inner>>,
}

impl InnerMut {
	fn pinned(&mut self) -> Pin<&mut Inner> {
		let ptr: *mut Inner = self.inner.get();
		unsafe { Pin::new_unchecked(&mut *ptr) }
	}
}

pub struct Reactor {
	inner: Rc<UnsafeCell<Inner>>,
}

impl Reactor {
	fn build() -> io::Result<Self> {
		let inner = Rc::new(UnsafeCell::new(Inner::build()?));
		Ok(Reactor {
			inner,
		})
	}

	pub fn new() -> io::Result<Self> {
		let reactor = Self::build()?;
		reactor.inner_mut().pinned().init();
		Ok(reactor)
	}

	fn inner_mut(&self) -> InnerMut {
		let inner = self.inner.clone();
		InnerMut {
			inner
		}
	}

	pub fn handle(&self) -> Handle {
		Handle(Rc::downgrade(&self.inner))
	}
}

impl tokio_executor::park::Park for Reactor {
	type Unpark = Unpark;

	type Error = io::Error;

	fn unpark(&self) -> Self::Unpark {
		Unpark(self.inner_mut().pinned().completion_state.park.unpark())
	}

	fn park(&mut self) -> Result<(), Self::Error> {
		self.inner_mut().pinned().park()
	}

	fn park_timeout(&mut self, duration: std::time::Duration) -> Result<(), Self::Error> {
		self.inner_mut().pinned().park_timeout(duration)
	}
}

impl fmt::Debug for Reactor {
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		write!(f, "Reactor {{..}}")
	}
}


#[derive(Clone)]
pub struct Handle(Weak<UnsafeCell<Inner>>);

impl Handle {
	fn inner_mut(&self) -> io::Result<InnerMut> {
		let inner = self.0.upgrade().ok_or_else(|| {
			io::Error::new(io::ErrorKind::Other, "uring reactor dead")
		})?;

		Ok(InnerMut { inner })
	}

	pub fn async_read<T, F>(&self, file: F, offset: u64, buf: T) -> AsyncRead<T, F>
	where
		T: AsMut<[u8]> + 'static,
		F: AsRawFd + 'static,
	{
		AsyncRead::new(self, file, offset, buf)
	}

	pub fn async_write<T: AsRef<[u8]> + 'static, F: AsRawFd + 'static>(&self, file: F, offset: u64, buf: T) -> AsyncWrite<T, F>
	where
		T: AsRef<[u8]> + 'static,
		F: AsRawFd + 'static,
	{
		AsyncWrite::new(self, file, offset, buf)
	}

	pub fn async_poll(&self, fd: RawFd, flags: io_uring::PollFlags) -> AsyncPoll {
		AsyncPoll::new(self, fd, flags)
	}
}

impl fmt::Debug for Handle {
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		write!(f, "Handle {{..}}")
	}
}
