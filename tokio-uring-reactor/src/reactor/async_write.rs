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

struct WriteContext<T: 'static, F: 'static> {
	iovec: [libc::iovec; 1],
	buf: T,
	file: F,
}

enum AsyncWriteState<T: 'static, F: 'static> {
	Pending(Registration<WriteContext<T, F>>),
	InitFailed(io::Error, T, F),
	Closed,
}

pub struct AsyncWrite<T: 'static, F: 'static>(AsyncWriteState<T, F>);

impl<T, F> AsyncWrite<T, F> {
	pub(super) fn new(handle: &Handle, file: F, offset: u64, buf: T) -> AsyncWrite<T, F>
	where
		T: AsRef<[u8]> + 'static,
		F: AsRawFd + 'static,
	{
		let fd = file.as_raw_fd();
		let mut im = match handle.inner_mut() {
			Err(e) => return AsyncWrite(AsyncWriteState::InitFailed(e, buf, file)),
			Ok(im) => im,
		};

		let rc = WriteContext {
			iovec: [ iovec_empty() ], // fill below
			buf,
			file,
		};
		// this "pins" buf, as the data is boxed
		let mut reg = Registration::new(rc);
		let queue_result = {
			let iovec = unsafe {
				let d = reg.data_mut();
				d.iovec[0] = iovec_from(d.buf.as_ref());
				&d.iovec
			};

			im.pinned().queue_async_write(fd, offset, iovec, reg.to_raw())
		};
		if let Err(e) = queue_result {
			let data = reg.abort().expect("registration data");
			return AsyncWrite(AsyncWriteState::InitFailed(e, data.buf, data.file));
		}
		AsyncWrite(AsyncWriteState::Pending(reg))
	}
}

impl<T: 'static, F: 'static> fmt::Debug for AsyncWrite<T, F> {
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		write!(f, "AsyncWrite(..)")
	}
}

impl<T: 'static, F: 'static> futures::Future for AsyncWrite<T, F> {
	type Item = (usize, T, F);
	type Error = (io::Error, T, F);

	fn poll(&mut self) -> futures::Poll<Self::Item, Self::Error> {
		match self.0 {
			AsyncWriteState::Pending(ref mut p) => {
				match p.poll() {
					futures::Async::NotReady => Ok(futures::Async::NotReady),
					futures::Async::Ready((r, d)) => {
						let result = if r.result < 0 {
							Err((io::Error::from_raw_os_error(r.result), d.buf, d.file))
						} else {
							Ok(futures::Async::Ready((r.result as usize, d.buf, d.file)))
						};
						std::mem::replace(&mut self.0, AsyncWriteState::Closed);
						result
					}
				}
			}
			_ => {
				match std::mem::replace(&mut self.0, AsyncWriteState::Closed) {
					AsyncWriteState::Pending(_) => unreachable!(),
					AsyncWriteState::InitFailed(e, buf, file) => Err((e, buf, file)),
					AsyncWriteState::Closed => panic!("already finished"),
				}
			}
		}
	}
}
