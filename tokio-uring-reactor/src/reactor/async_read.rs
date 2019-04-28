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

struct ReadContext<T: 'static, F: 'static> {
	iovec: [libc::iovec; 1],
	buf: T,
	file: F,
}

enum AsyncReadState<T: 'static, F: 'static> {
	Pending(Registration<ReadContext<T, F>>),
	InitFailed(io::Error, T, F),
	Closed,
}

pub struct AsyncRead<T: 'static, F: 'static>(AsyncReadState<T, F>);

impl<T, F> AsyncRead<T, F> {
	pub(super) fn new(handle: &Handle, file: F, offset: u64, buf: T) -> AsyncRead<T, F>
	where
		T: AsMut<[u8]> + 'static,
		F: AsRawFd + 'static,
	{
		let fd = file.as_raw_fd();
		let mut im = match handle.inner_mut() {
			Err(e) => return AsyncRead(AsyncReadState::InitFailed(e, buf, file)),
			Ok(im) => im,
		};

		let rc = ReadContext {
			iovec: [ iovec_empty() ], // fill below
			buf,
			file,
		};
		// this "pins" buf, as the data is boxed
		let mut reg = Registration::new(rc);
		let queue_result = {
			let iovec = unsafe {
				let d = reg.data_mut();
				d.iovec[0] = iovec_from(d.buf.as_mut());
				&d.iovec
			};

			im.pinned().queue_async_read(fd, offset, iovec, reg.to_raw())
		};
		if let Err(e) = queue_result {
			let data = reg.abort().expect("registration data");
			return AsyncRead(AsyncReadState::InitFailed(e, data.buf, data.file));
		}
		AsyncRead(AsyncReadState::Pending(reg))
	}
}

impl<T: 'static, F: 'static> fmt::Debug for AsyncRead<T, F> {
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		write!(f, "AsyncRead(..)")
	}
}

impl<T: 'static, F: 'static> futures::Future for AsyncRead<T, F> {
	type Item = (usize, T, F);
	type Error = (io::Error, T, F);

	fn poll(&mut self) -> futures::Poll<Self::Item, Self::Error> {
		match self.0 {
			AsyncReadState::Pending(ref mut p) => {
				match p.poll() {
					futures::Async::NotReady => Ok(futures::Async::NotReady),
					futures::Async::Ready((r, d)) => {
						let result = if r.result < 0 {
							Err((io::Error::from_raw_os_error(r.result), d.buf, d.file))
						} else {
							Ok(futures::Async::Ready((r.result as usize, d.buf, d.file)))
						};
						std::mem::replace(&mut self.0, AsyncReadState::Closed);
						result
					}
				}
			},
			_ => {
				match std::mem::replace(&mut self.0, AsyncReadState::Closed) {
					AsyncReadState::Pending(_) => unreachable!(),
					AsyncReadState::InitFailed(e, buf, file) => Err((e, buf, file)),
					AsyncReadState::Closed => panic!("already finished"),
				}
			}
		}
	}
}
