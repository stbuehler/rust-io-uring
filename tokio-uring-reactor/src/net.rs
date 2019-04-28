use std::net;
use std::io;
use std::os::unix::io::{RawFd, AsRawFd};

use crate::Handle;

#[derive(Debug)]
pub struct TcpListener(net::TcpListener);

impl TcpListener {
	pub fn incoming(self, handle: &Handle) -> Incoming {
		let fd = self.0.as_raw_fd();
		Incoming {
			inner: self,
			blocked: true, // poll first
			poll: handle.async_poll(fd, io_uring::PollFlags::IN),
		}
	}
}

impl From<net::TcpListener> for TcpListener {
	fn from(l: net::TcpListener) -> Self {
		crate::set_non_block(l.as_raw_fd());
		TcpListener(l)
	}
}

#[must_use = "streams do nothing unless polled"]
#[derive(Debug)]
pub struct Incoming {
	inner: TcpListener,
	blocked: bool,
	poll: crate::reactor::AsyncPoll,
}

impl futures::Stream for Incoming {
	type Item = (TcpStream, net::SocketAddr);
	type Error = io::Error;

	fn poll(&mut self) -> futures::Poll<Option<Self::Item>, Self::Error> {
		loop {
			if !self.blocked {
				match self.inner.0.accept() {
					Ok((s, a)) => return Ok(futures::Async::Ready(Some((
						TcpStream(s),
						a,
					)))),
					Err(e) => {
						if e.kind() == io::ErrorKind::Interrupted {
							continue; // again
						} else if e.kind() == io::ErrorKind::WouldBlock {
							self.blocked = true;
						} else {
							return Err(e);
						}
					}
				}
			}
			match self.poll.poll()? {
				futures::Async::NotReady => return Ok(futures::Async::NotReady),
				futures::Async::Ready(None) => unreachable!(),
				futures::Async::Ready(Some(_events)) => {
					// println!("Incoming events: {:?}", _events);
					self.blocked = false;
					// try loop again
				},
			}
		}
	}
}

#[derive(Debug)]
pub struct TcpStream(net::TcpStream);

impl AsRawFd for TcpStream {
	fn as_raw_fd(&self) -> RawFd {
		self.0.as_raw_fd()
	}
}

impl crate::io::SocketRead for TcpStream {}
impl crate::io::SocketWrite for TcpStream {}
