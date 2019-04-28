use std::io;
use std::os::unix::io::{AsRawFd, RawFd};
use std::rc::Rc;

use crate::reactor::{
	Handle,
	AsyncRead,
	AsyncWrite,
};

pub trait SocketRead: AsRawFd + Sized {
	fn read<T: AsMut<[u8]>>(self, handle: &Handle, buf: T) -> AsyncRead<T, Self> {
		handle.async_read(self, 0, buf)
	}

	fn split(self) -> (SplitRead<Self>, SplitWrite<Self>)
	where
		Self: SocketWrite,
	{
		split(self)
	}
}

pub trait SocketWrite: AsRawFd + Sized {
	fn write<T: AsRef<[u8]>>(self, handle: &Handle, buf: T) -> io::Result<AsyncWrite<T, Self>> {
		handle.async_write(self, 0, buf)
	}
}

pub fn split<T>(rw: T) -> (SplitRead<T>, SplitWrite<T>) {
	let rw = Rc::new(rw);
	(SplitRead(rw.clone()), SplitWrite(rw))
}

#[derive(Debug)]
pub struct SplitRead<T>(Rc<T>);

impl<T: AsRawFd> AsRawFd for SplitRead<T> {
	fn as_raw_fd(&self) -> RawFd {
		self.0.as_raw_fd()
	}
}

impl<T: SocketRead> SocketRead for SplitRead<T> {
}

#[derive(Debug)]
pub struct SplitWrite<T>(Rc<T>);

impl<T: AsRawFd> AsRawFd for SplitWrite<T> {
	fn as_raw_fd(&self) -> RawFd {
		self.0.as_raw_fd()
	}
}

impl<T: SocketWrite> SocketWrite for SplitWrite<T> {
}
