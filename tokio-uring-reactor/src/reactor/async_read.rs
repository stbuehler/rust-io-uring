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
pub struct AsyncReadError<T, F> {
	pub error: io::Error,
	pub buffer: T,
	pub file: F,
}

impl<T, F> From<AsyncReadError<T, F>> for io::Error {
	fn from(e: AsyncReadError<T, F>) -> io::Error {
		e.error
	}
}

struct Context<T: 'static, F: 'static> {
	iovec: [libc::iovec; 1],
	buffer: T,
	file: F,
}

impl<T: 'static, F: 'static> Context<T, F> {
	fn with_error(self, error: io::Error) -> AsyncReadError<T, F> {
		AsyncReadError {
			error,
			buffer: self.buffer,
			file: self.file,
		}
	}
}

enum State<T: 'static, F: 'static> {
	Pending(Registration<Context<T, F>>),
	InitFailed(AsyncReadError<T, F>),
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

pub struct AsyncRead<T: 'static, F: 'static>(State<T, F>);

impl<T, F> AsyncRead<T, F> {
	pub(super) fn new(handle: &Handle, file: F, offset: u64, buffer: T) -> AsyncRead<T, F>
	where
		T: AsMut<[u8]> + 'static,
		F: AsRawFd + 'static,
	{
		let fd = file.as_raw_fd();
		let context = Context {
			iovec: [ iovec_empty() ], // fill below
			buffer,
			file,
		};

		let mut im = match handle.inner_mut() {
			Err(e) => return AsyncRead(State::InitFailed(context.with_error(e))),
			Ok(im) => im,
		};

		// this "pins" buf, as the data is boxed
		let mut reg = Registration::new(context);
		let queue_result = {
			let iovec = unsafe {
				let d = reg.data_mut();
				d.iovec[0] = iovec_from(d.buffer.as_mut());
				&d.iovec
			};

			im.pinned().queue_async_read(fd, offset, iovec, reg.to_raw())
		};
		if let Err(e) = queue_result {
			let context = reg.abort().expect("registration context");
			return AsyncRead(State::InitFailed(context.with_error(e)));
		}
		AsyncRead(State::Pending(reg))
	}
}

impl<T: 'static, F: 'static> fmt::Debug for AsyncRead<T, F> {
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		f.debug_tuple("AsyncRead").field(&self.0).finish()
	}
}

impl<T: 'static, F: 'static> futures::Future for AsyncRead<T, F> {
	type Item = (usize, T, F);
	type Error = AsyncReadError<T, F>;

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
			},
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

#[cfg(feature = "nightly-async")]
use std::{
	pin::Pin,
	task,
	future::Future,
	task::Poll,
};

#[cfg(feature = "nightly-async")]
impl<T: Unpin + 'static, F: Unpin + 'static> Future for AsyncRead<T, F> {
	type Output = Result<(usize, T, F), AsyncReadError<T, F>>;

	fn poll(mut self: Pin<&mut Self>, ctx: &mut task::Context<'_>) -> Poll<Self::Output> {
		let this: &mut Self = &mut *self;
		match this.0 {
			State::Pending(ref mut p) => {
				match p.poll_async(ctx.waker()) {
					Poll::Pending => Poll::Pending,
					Poll::Ready((r, context)) => {
						let result = if r.result < 0 {
							Err(context.with_error(io::Error::from_raw_os_error(-r.result)))
						} else {
							Ok((r.result as usize, context.buffer, context.file))
						};
						std::mem::replace(&mut this.0, State::Closed);
						Poll::Ready(result)
					}
				}
			},
			_ => {
				match std::mem::replace(&mut this.0, State::Closed) {
					State::Pending(_) => unreachable!(),
					State::InitFailed(e) => Poll::Ready(Err(e)),
					State::Closed => panic!("already finished"),
				}
			}
		}
	}
}
