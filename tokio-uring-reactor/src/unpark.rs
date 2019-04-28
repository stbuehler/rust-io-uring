use std::io;
use std::fs::File;
use std::os::unix::io::{
	FromRawFd,
	AsRawFd,
	RawFd,
};
use std::sync::{
	atomic::{
		AtomicBool,
		Ordering,
	},
	Arc,
	Weak,
};

struct Shared {
	write_fd: File,
	pending: AtomicBool,
	entered: AtomicBool,
}

pub struct Unpark {
	shared: Weak<Shared>,
}

impl Unpark {
	pub fn unpark(&self) {
		const DATA: &'static [u8] = b"u";

		let shared = match self.shared.upgrade() {
			None => return,
			Some(shared) => shared,
		};

		if shared.pending.swap(true, Ordering::AcqRel) {
			// already pending
			return;
		}
		if shared.entered.load(Ordering::SeqCst) {
			unsafe {
				libc::write(
					shared.write_fd.as_raw_fd(),
					DATA.as_ptr() as *const libc::c_void,
					DATA.len(),
				);
			}
		}
	}
}

pub struct Park {
	shared: Arc<Shared>,
	read_fd: File,
}

impl Park {
	pub fn new() -> io::Result<Self> {
		let mut fds = [-1i32; 2];
		let res = unsafe {
			libc::pipe2(
				(&mut fds[..]).as_mut_ptr(),
				libc::O_CLOEXEC, // | libc::O_NONBLOCK,
			)
		};
		if 0 != res {
			return Err(io::Error::last_os_error());
		}
		let read_fd = unsafe { File::from_raw_fd(fds[0]) };
		let write_fd = unsafe { File::from_raw_fd(fds[1]) };
		crate::set_non_block(fds[1]); // only write end non blocking
		Ok(Park {
			shared: Arc::new(Shared {
				write_fd,
				pending: AtomicBool::new(false),
				entered: AtomicBool::new(false),
			}),
			read_fd,
		})
	}

	pub fn enter(&self) -> ParkEntered
	{
		self.shared.entered.store(true, Ordering::SeqCst);

		let allow_wait = !self.shared.pending.load(Ordering::SeqCst);

		ParkEntered {
			allow_wait,
			park: self,
		}
	}

	pub fn unpark(&self) -> Unpark {
		Unpark {
			shared: Arc::downgrade(&self.shared),
		}
	}

	pub fn pending(&self) -> bool {
		self.shared.pending.load(Ordering::Relaxed)
	}

	pub fn clear_unpark(&self) {
		self.shared.pending.store(false, Ordering::Relaxed);
	}

	pub fn clear_event(&self) {
		let mut buf = [0u8; 16];

		unsafe {
			libc::read(
				self.read_fd.as_raw_fd(),
				buf[..].as_mut_ptr() as *mut libc::c_void,
				buf.len(),
			);
		}
	}
}

pub struct ParkEntered<'a> {
	pub allow_wait: bool,
	park: &'a Park,
}

impl Drop for ParkEntered<'_> {
	fn drop(&mut self) {
		self.park.shared.entered.store(false, Ordering::Relaxed);
		self.park.clear_unpark();
	}
}

impl AsRawFd for Park {
	fn as_raw_fd(&self) -> RawFd {
		self.read_fd.as_raw_fd()
	}
}
