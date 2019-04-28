mod reactor;
mod registration;
mod unpark;
pub mod io;
pub mod net;

pub use crate::{
	reactor::{
		Reactor,
		Handle,
		Unpark,
	},
};

pub fn with_default<F, R>(_handle: &Handle, enter: &mut tokio_executor::Enter, f: F) -> R
where
	F: FnOnce(&mut tokio_executor::Enter) -> R,
{
	// TODO: some enter construction?
	f(enter)
}

fn set_non_block(fd: libc::c_int) {
	unsafe {
		let flags = libc::fcntl(fd, libc::F_GETFL, 0);
		if 0 == flags & libc::O_NONBLOCK {
			libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
		}
	}
}
