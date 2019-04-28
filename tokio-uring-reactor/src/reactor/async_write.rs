use std::{
	fmt,
	io,
	os::unix::io::{AsRawFd},
};

use crate::{
	reactor::{
		Handle,
		iovec_empty,
		iovec_from,
	},
	registration::{
		Registration,
	},
};

// #[non_exhaustive] TODO ?
pub struct AsyncWriteError<T, F> {
	pub error: io::Error,
	pub buffer: T,
	pub file: F,
}

impl<T, F> From<AsyncWriteError<T, F>> for io::Error {
	fn from(e: AsyncWriteError<T, F>) -> io::Error {
		e.error
	}
}

struct Context<T: 'static, F: 'static> {
	iovec: [libc::iovec; 1],
	buffer: T,
	file: F,
}

impl<T: 'static, F: 'static> Context<T, F> {
	fn with_error(self, error: io::Error) -> AsyncWriteError<T, F> {
		AsyncWriteError {
			error,
			buffer: self.buffer,
			file: self.file,
		}
	}
}

enum State<T: 'static, F: 'static> {
	Pending(Registration<Context<T, F>>),
	InitFailed(AsyncWriteError<T, F>),
	Closed,
}

impl<T: 'static, F: 'static> fmt::Debug for State<T, F> {
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		match self {
			State::Pending(ref p) => f.debug_tuple("Pending").field(p).finish(),
			State::InitFailed(ref e) => f.debug_tuple("InitFailed").field(&e.error).finish(),
			State::Closed => f.debug_tuple("Closed").finish(),
		}
	}
}

pub struct AsyncWrite<T: 'static, F: 'static>(State<T, F>);

impl<T, F> AsyncWrite<T, F> {
	pub(super) fn new(handle: &Handle, file: F, offset: u64, buffer: T) -> AsyncWrite<T, F>
	where
		T: AsRef<[u8]> + 'static,
		F: AsRawFd + 'static,
	{
		let fd = file.as_raw_fd();
		let context = Context {
			iovec: [ iovec_empty() ], // fill below
			buffer,
			file,
		};

		let mut im = match handle.inner_mut() {
			Err(e) => return AsyncWrite(State::InitFailed(context.with_error(e))),
			Ok(im) => im,
		};

		// this "pins" buf, as the data is boxed
		let mut reg = Registration::new(context);
		let queue_result = {
			let iovec = unsafe {
				let d = reg.data_mut();
				d.iovec[0] = iovec_from(d.buffer.as_ref());
				&d.iovec
			};

			im.pinned().queue_async_write(fd, offset, iovec, reg.to_raw())
		};
		if let Err(e) = queue_result {
			let context = reg.abort().expect("registration context");
			return AsyncWrite(State::InitFailed(context.with_error(e)));
		}
		AsyncWrite(State::Pending(reg))
	}
}

impl<T: 'static, F: 'static> fmt::Debug for AsyncWrite<T, F> {
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		f.debug_tuple("AsyncWrite").field(&self.0).finish()
	}
}

impl<T: 'static, F: 'static> futures::Future for AsyncWrite<T, F> {
	type Item = (usize, T, F);
	type Error = AsyncWriteError<T, F>;

	fn poll(&mut self) -> futures::Poll<Self::Item, Self::Error> {
		match self.0 {
			State::Pending(ref mut p) => {
				match p.poll() {
					futures::Async::NotReady => Ok(futures::Async::NotReady),
					futures::Async::Ready((r, context)) => {
						let result = if r.result < 0 {
							Err(context.with_error(io::Error::from_raw_os_error(-r.result)))
						} else {
							Ok(futures::Async::Ready((r.result as usize, context.buffer, context.file)))
						};
						std::mem::replace(&mut self.0, State::Closed);
						result
					}
				}
			}
			_ => {
				match std::mem::replace(&mut self.0, State::Closed) {
					State::Pending(_) => unreachable!(),
					State::InitFailed(e) => Err(e),
					State::Closed => panic!("already finished"),
				}
			}
		}
	}
}
