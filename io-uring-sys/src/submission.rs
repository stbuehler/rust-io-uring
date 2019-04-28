// additional code that makes it easier to handle submissions

use crate::*;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
#[repr(u8)]
pub enum IoPriorityLevel {
	Level0 = 0,
	Level1 = 1,
	Level2 = 2,
	Level3 = 3,
	Level4 = 4,
	Level5 = 5,
	Level6 = 6,
	Level7 = 7,
}

impl IoPriorityLevel {
	pub fn try_from(v: u8) -> Option<Self> {
		Some(match v {
			0 => IoPriorityLevel::Level0,
			1 => IoPriorityLevel::Level1,
			2 => IoPriorityLevel::Level2,
			3 => IoPriorityLevel::Level3,
			4 => IoPriorityLevel::Level4,
			5 => IoPriorityLevel::Level5,
			6 => IoPriorityLevel::Level6,
			7 => IoPriorityLevel::Level7,
			_ => return None,
		})
	}
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum IoPriority {
	None,
	Realtime(IoPriorityLevel),
	BestEffort(IoPriorityLevel),
	Idle,
}

impl IoPriority {
	pub fn try_from(e: EncodedIoPriority) -> Option<Self> {
		Some(match e.0 >> 13 {
			0 => IoPriority::None,
			1 => IoPriority::Realtime(IoPriorityLevel::try_from(e.0 as u8)?),
			2 => IoPriority::BestEffort(IoPriorityLevel::try_from(e.0 as u8)?),
			3 => IoPriority::Idle,
			_ => return None,
		})
	}
}

impl Default for IoPriority {
	fn default() -> Self {
		IoPriority::None
	}
}

impl Into<EncodedIoPriority> for IoPriority {
	fn into(self) -> EncodedIoPriority {
		EncodedIoPriority(match self {
			IoPriority::None => 0 << 13,
			IoPriority::Realtime(l) => (1 << 13) | ((l as u8) as u16),
			IoPriority::BestEffort(l) => (2 << 13) | ((l as u8) as u16),
			IoPriority::Idle => 3 << 13,
		})
	}
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum FileDescriptor {
	/// standard file descriptor
	FD(i32),
	/// index into previously registered list of fds
	Fixed(u32),
}

impl SubmissionEntry {
	pub fn clear(&mut self) {
		unsafe {
			*self = core::mem::zeroed();
		}
	}

	fn iov(&mut self, op: Operation, prio: IoPriority, fd: FileDescriptor, offset: u64, flags: ReadWriteFlags, iov: *const [libc::iovec]) {
		self.opcode = op.into();
		self.flags = Default::default();
		self.ioprio = prio.into();
		match fd {
			FileDescriptor::FD(fd) => self.fd = fd,
			FileDescriptor::Fixed(ndx) => {
				self.flags |= SubmissionEntryFlags::FIXED_FILE;
				self.fd = ndx as i32;
			}
		}
		self.off = offset;
		self.addr = unsafe { (*iov).as_ptr() } as usize as u64;
		self.len = unsafe { (*iov).len() } as u32;
		self.op_flags = flags.into();
		unsafe {
			self.extra.fixed.buf_index = 0;
		}
	}

	// iov needs to live until operation is completed! (as the kernel might submit the request "async")
	pub unsafe fn readv(&mut self, prio: IoPriority, fd: FileDescriptor, offset: u64, flags: ReadWriteFlags, iov: *const [libc::iovec]) {
		self.iov(Operation::Readv, prio, fd, offset, flags, iov);
	}

	// iov needs to live until operation is completed! (as the kernel might submit the request "async")
	pub unsafe fn writev(&mut self, prio: IoPriority, fd: FileDescriptor, offset: u64, flags: ReadWriteFlags, iov: *const [libc::iovec]) {
		self.iov(Operation::Writev, prio, fd, offset, flags, iov);
	}

	fn io_fixed(&mut self, op: Operation, prio: IoPriority, fd: FileDescriptor, offset: u64, flags: ReadWriteFlags, buf_index: u16, buf: *const [u8]) {
		self.opcode = op.into();
		self.flags = Default::default();
		self.ioprio = prio.into();
		match fd {
			FileDescriptor::FD(fd) => self.fd = fd,
			FileDescriptor::Fixed(ndx) => {
				self.flags |= SubmissionEntryFlags::FIXED_FILE;
				self.fd = ndx as i32;
			}
		}
		self.off = offset;
		self.addr = unsafe { (*buf).as_ptr() } as usize as u64;
		self.len = unsafe { (*buf).len() } as u32;
		self.op_flags = flags.into();
		unsafe {
			self.extra.fixed.buf_index = buf_index;
		}
	}

	// buf must be a sub-slice of the buffer registered at the given index
	pub unsafe fn read_fixed(&mut self, prio: IoPriority, fd: FileDescriptor, offset: u64, flags: ReadWriteFlags, buf_index: u16, buf: *const [u8]) {
		self.io_fixed(Operation::ReadFixed, prio, fd, offset, flags, buf_index, buf);
	}

	// buf must be a sub-slice of the buffer registered at the given index
	pub unsafe fn write_fixed(&mut self, prio: IoPriority, fd: FileDescriptor, offset: u64, flags: ReadWriteFlags, buf_index: u16, buf: *const [u8]) {
		self.io_fixed(Operation::WriteFixed, prio, fd, offset, flags, buf_index, buf);
	}

	pub fn fsync_full(&mut self, fd: FileDescriptor, flags: FsyncFlags) {
		self.fsync(fd, flags, 0, 0);
	}

	// if offset + len == 0 it syncs until end of file
	// right now it seems to require FsyncFlags::DATASYNC to be set.
	pub fn fsync(&mut self, fd: FileDescriptor, flags: FsyncFlags, offset: u64, len: u32) {
		self.opcode = Operation::Fsync.into();
		self.flags = Default::default();
		self.ioprio = EncodedIoPriority(0);
		match fd {
			FileDescriptor::FD(fd) => self.fd = fd,
			FileDescriptor::Fixed(ndx) => {
				self.flags |= SubmissionEntryFlags::FIXED_FILE;
				self.fd = ndx as i32;
			}
		}
		self.off = offset;
		self.addr = 0;
		self.len = len;
		self.op_flags = flags.into();
		unsafe {
			self.extra.fixed.buf_index = 0;
		}
	}

	// The CQE `res` will contain the mask with "ready" eventy flags
	pub fn poll_add(&mut self, fd: FileDescriptor, flags: PollFlags) {
		self.opcode = Operation::PollAdd.into();
		self.flags = Default::default();
		self.ioprio = EncodedIoPriority(0);
		match fd {
			FileDescriptor::FD(fd) => self.fd = fd,
			FileDescriptor::Fixed(ndx) => {
				self.flags |= SubmissionEntryFlags::FIXED_FILE;
				self.fd = ndx as i32;
			}
		}
		self.off = 0;
		self.addr = 0;
		self.len = 0;
		self.op_flags = flags.into();
		unsafe {
			self.extra.fixed.buf_index = 0;
		}
	}

	// the PollRemove operation will still complete (possibly with an empty mask)
	pub fn poll_remove(&mut self, match_user_data: u64) {
		self.opcode = Operation::PollRemove.into();
		self.flags = Default::default();
		self.ioprio = EncodedIoPriority(0);
		self.fd = 0;
		self.off = 0;
		self.addr = match_user_data;
		self.len = 0;
		self.op_flags = 0u32.into();
		unsafe {
			self.extra.fixed.buf_index = 0;
		}
	}
}
