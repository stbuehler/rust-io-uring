use std::fmt;
use std::io;

pub struct MappedMemory {
	addr: *mut libc::c_void,
	len: usize,
}

impl MappedMemory {
	pub fn map(fd: libc::c_int, offset: i64, len: usize) -> io::Result<Self> {
		let addr = unsafe { libc::mmap(
			0 as *mut _,
			len,
			libc::PROT_READ | libc::PROT_WRITE,
			libc::MAP_SHARED | libc::MAP_POPULATE,
			fd,
			offset,
		) };
		if addr == libc::MAP_FAILED {
			Err(io::Error::last_os_error())
		} else {
			Ok(MappedMemory {
				addr,
				len,
			})
		}
	}

	pub fn as_mut_ptr(&self) -> *mut libc::c_void {
		self.addr
	}

/*
	pub fn len(&self) -> usize {
		self.len
	}
*/

	pub fn get_field<T>(&self, offset: u32) -> *mut T {
		(self.addr as usize + (offset as usize)) as *mut T
	}
}

impl Drop for MappedMemory {
	fn drop(&mut self) {
		if self.len != 0 {
			if 0 != unsafe { libc::munmap(self.addr, self.len) } {
				log::error!(
					"munmap(0x{:x}, {}) failed: {}",
					self.addr as usize,
					self.len,
					std::io::Error::last_os_error(),
				);
			}
		}
	}
}

impl fmt::Debug for MappedMemory {
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		write!(f, "MappedMemory{{addr: 0x{:x}, len: {}}}", self.addr as usize, self.len)
	}
}
