use std::{
	io,
	os::unix::io::RawFd,
};

use crate::{
	reactor::{
		Handle,
	},
	registration::{
		Registration,
	},
};

// TODO: Dropping AsyncPoll should trigger a POLL_DEL
#[derive(Debug)]
pub struct AsyncPoll {
	handle: Handle,
	fd: RawFd,
	active: bool,
	flags: io_uring::PollFlags,
	registration: Registration<()>,
}

impl AsyncPoll {
	pub fn new(handle: &Handle, fd: RawFd, flags: io_uring::PollFlags) -> AsyncPoll {
		let registration = Registration::new(());

		AsyncPoll {
			active: false,
			handle: handle.clone(),
			fd,
			flags,
			registration,
		}
	}
}

impl futures::Stream for AsyncPoll {
	type Item = io_uring::PollFlags;
	type Error = io::Error;

	fn poll(&mut self) -> futures::Poll<Option<Self::Item>, Self::Error> {
		if !self.active {
			// println!("Register fd {} for events {:?}", self.fd, self.flags);
			let mut im = self.handle.inner_mut()?;
			im.pinned().queue_async_poll(self.fd, self.flags, self.registration.to_raw())?;
			self.active = true;
			self.registration.track();
			return Ok(futures::Async::NotReady);
		}
		match self.registration.poll_stream_and_reset() {
			futures::Async::NotReady => Ok(futures::Async::NotReady),
			futures::Async::Ready(r) => {
				self.active = false;
				if r.result < 0 {
					return Err(io::Error::from_raw_os_error(r.result));
				}
				let flags = io_uring::PollFlags::from_bits_truncate(r.result as u16);
				Ok(futures::Async::Ready(Some(flags)))
			}
		}
	}
}
