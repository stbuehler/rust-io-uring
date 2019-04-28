#![no_std]

mod macros;

mod submission;
pub use submission::{
	IoPriorityLevel,
	IoPriority,
	FileDescriptor,
};

use bitflags::bitflags;

use core::mem::size_of;
use core::fmt;
use libc::{
	c_int,
	c_long,
	c_uint,
	c_void,
};

static_assert!(
	size_of::<SetupParameters>() == 120,
	size_of::<SubmissionQueueRingOffsets>() == 40,
	size_of::<CompletionQueueRingOffsets>() == 40,
	size_of::<SubmissionEntry>() == 64,
	size_of::<SubmissionEntryOperationFlags>() == 4,
	size_of::<SubmissionEntryExtraData>() == 24,
	size_of::<CompletionEntry>() == 16,
	true
);

#[cfg(all(target_os = "linux", any(target_arch = "x86", target_arch = "x86_64")))]
#[allow(non_upper_case_globals)]
mod syscalls {
	pub const SYS_io_uring_setup: libc::c_long = 425;
	pub const SYS_io_uring_enter: libc::c_long = 426;
	pub const SYS_io_uring_register: libc::c_long = 427;
}

pub unsafe fn io_uring_setup(entries: u32, params: *mut SetupParameters) -> c_int {
	libc::syscall(
		syscalls::SYS_io_uring_setup,
		entries as c_long,
		params as usize as c_long,
	) as c_int
}

pub unsafe fn io_uring_enter(fd: c_int, to_submit: c_uint, min_complete: c_uint, flags: c_uint, sig: *const libc::sigset_t) -> c_int {
	libc::syscall(
		syscalls::SYS_io_uring_enter,
		fd as c_long,
		to_submit as c_long,
		min_complete as c_long,
		flags as c_long,
		sig as usize as c_long,
		core::mem::size_of::<libc::sigset_t>() as c_long,
	) as c_int
}

pub unsafe fn io_uring_register(fd: c_int, opcode: c_uint, arg: *const c_void, nr_args: c_uint) -> c_int {
	libc::syscall(
		syscalls::SYS_io_uring_register,
		fd as c_long,
		opcode as c_long,
		arg as usize as c_long,
		nr_args as c_long,
	) as c_int
}

bitflags! {
	#[derive(Default)]
	pub struct EnterFlags: u32 {
		/// `IORING_ENTER_GETEVENTS`
		const GETEVENTS = (1 << 0);
		/// `IORING_ENTER_SQ_WAKEUP`
		const SQ_WAKEUP = (1 << 1);

		// don't truncate any bits
		#[doc(hidden)]
		const _ALL = !0;
	}
}

#[derive(Clone, Copy, Debug)]
pub struct RegisterOpCode(pub u32);

impl RegisterOpCode {
	/// `IORING_REGISTER_BUFFERS`
	pub const REGISTER_BUFFERS: Self = Self(0);
	/// `IORING_UNREGISTER_BUFFERS`
	pub const UNREGISTER_BUFFERS: Self = Self(1);
	/// `IORING_REGISTER_FILES`
	pub const REGISTER_FILES: Self = Self(2);
	/// `IORING_UNREGISTER_FILES`
	pub const UNREGISTER_FILES: Self = Self(3);
}

/// Passed in for io_uring_setup(2). Copied back with updated info on
/// success
///
/// C: `struct io_uring_params`
#[derive(Clone, Copy, Default, Debug)]
#[repr(C)]
pub struct SetupParameters {
	/// (output) allocated entries in submission queue
	///
	/// (both ring index `array` and separate entry array at
	/// `SUBMISSION_QUEUE_ENTRIES_OFFSET`).
	pub sq_entries: u32,
	/// (output) allocated entries in completion queue
	pub cq_entries: u32,
	/// (input)
	pub flags: SetupFlags,
	/// (input) used if SQ_AFF and SQPOLL flags are active to pin poll
	/// thread to specific cpu
	///
	/// right now always checked in kernel for "possible cpu".
	pub sq_thread_cpu: u32,
	/// (input) used if SQPOLL flag is active; timeout in milliseconds
	/// until kernel poll thread goes to sleep.
	pub sq_thread_idle: u32,
	// reserved
	_reserved: [u32; 5],
	/// (output) submission queue ring data field offsets
	pub sq_off: SubmissionQueueRingOffsets,
	/// (output) completion queue ring data field offsets
	pub cq_off: CompletionQueueRingOffsets,
}

impl SetupParameters {
	/// `IORING_OFF_SQ_RING`: mmap offset for submission queue ring
	pub const SUBMISSION_QUEUE_RING_OFFSET: i64 = 0;
	/// `IORING_OFF_CQ_RING`: mmap offset for completion queue ring
	pub const COMPLETION_QUEUE_RING_OFFSET: i64 = 0x8000000;
	/// `IORING_OFF_SQES`: mmap offset for submission entries
	pub const SUBMISSION_QUEUE_ENTRIES_OFFSET: i64 = 0x10000000;

	pub fn new(flags: SetupFlags) -> Self {
		Self {
			flags,
			..Self::default()
		}
	}
}


bitflags! {
	/// io_uring_setup() flags
	#[derive(Default)]
	pub struct SetupFlags: u32 {
		/// `IORING_SETUP_IOPOLL`: io_context is polled
		const IOPOLL = (1 << 0);

		/// `IORING_SETUP_SQPOLL`: SQ poll thread
		const SQPOLL = (1 << 1);

		/// `IORING_SETUP_SQ_AFF`: sq_thread_cpu is valid
		const SQ_AFF = (1 << 2);

		// don't truncate any bits
		#[doc(hidden)]
		const _ALL = !0;
	}
}

/// Offset to various struct members in mmap() at offset
/// `SUBMISSION_QUEUE_RING_OFFSET`
///
/// C: `struct io_sqring_offsets`
#[derive(Clone, Copy, Default, Debug)]
#[repr(C)]
pub struct SubmissionQueueRingOffsets {
	/// member type: AtomicU32; index into `self.array` (after `self.ring_mask` is applied)
	///
	/// incremented by kernel after entry at `head` was processed.
	///
	/// pending submissions: [head..tail]
	pub head: u32,
	/// member type: AtomicU32; index into `self.array` (after `self.ring_mask` is applied)
	///
	/// modified by user space when new entry was queued; points to next
	/// entry user space is going to fill.
	pub tail: u32,
	/// member type: (const) u32
	///
	/// value `value_at(self.ring_entries) - 1`
	///
	/// mask for indices at `head` and `tail` (don't delete masked bits!
	/// `head` and `tail` can point to the same entry, but if they are
	/// not exactly equal it implies the ring is full, and if they are
	/// exactly equal the ring is empty.)
	pub ring_mask: u32,
	/// member type: (const) u32; value same as SetupParameters.sq_entries, power of 2.
	pub ring_entries: u32,
	/// member type: (atomic) SubmissionQueueFlags
	pub flags: u32,
	/// member type: AtomicU32
	///
	/// number of (invalid) entries that were dropped; entries are
	/// invalid if there index (in `self.array`) is out of bounds.
	pub dropped: u32,
	/// member type: [u32] (index array into array of `SubmissionEntry`s
	/// at offset `SUBMISSION_QUEUE_ENTRIES_OFFSET` in mmap())
	pub array: u32,
	// reserved
	_reserved: [u32; 3],
}

bitflags! {
	#[derive(Default)]
	pub struct SubmissionQueueFlags: u32 {
		/// `IORING_SQ_NEED_WAKEUP`: needs io_uring_enter wakeup
		///
		/// set by kernel poll thread when it goes sleeping, and reset
		/// on wakeup
		const NEED_WAKEUP = (1 << 0);

		// don't truncate any bits
		#[doc(hidden)]
		const _ALL = !0;
	}
}

/// Offset to various struct members in mmap() at offset
/// `COMPLETION_QUEUE_RING_OFFSET`
///
/// C: `struct io_cqring_offsets`
#[derive(Clone, Copy, Default, Debug)]
#[repr(C)]
pub struct CompletionQueueRingOffsets {
	/// member type: AtomicU32; index into `self.cqes` (after `self.ring_mask` is applied)
	///
	/// incremented by user space after entry at `head` was processed.
	///
	/// available entries for processing: [head..tail]
	pub head: u32,
	/// member type: AtomicU32; index into `self.cqes` (after `self.ring_mask` is applied)
	///
	/// modified by kernel when new entry was created; points to next
	/// entry kernel is going to fill.
	pub tail: u32,
	/// member type: (const) u32
	///
	/// value `value_at(self.ring_entries) - 1`
	///
	/// mask for indices at `head` and `tail` (don't delete masked bits!
	/// `head` and `tail` can point to the same entry, but if they are
	/// not exactly equal it implies the ring is full, and if they are
	/// exactly equal the ring is empty.)
	pub ring_mask: u32,
	/// member type: (const) u32; value same as SetupParameters.cq_entries, power of 2.
	pub ring_entries: u32,
	/// member type: AtomicU32
	///
	/// incremented by the kernel every time it failed to queue a
	/// completion event because the ring was full.
	pub overflow: u32,
	/// member type: [CompletionEntry; self.ring_entries]
	pub cqes: u32,
	// reserved
	_reserved: [u64; 2],
}

/// C: `struct io_uring_sqe`
#[repr(C)]
#[derive(Debug)]
pub struct SubmissionEntry {
	pub opcode: RawOperation,
	pub flags: SubmissionEntryFlags,
	pub ioprio: EncodedIoPriority,
	pub fd: i32,
	pub off: u64,
	pub addr: u64,
	pub len: u32,
	pub op_flags: SubmissionEntryOperationFlags,
	pub user_data: u64,
	pub extra: SubmissionEntryExtraData,
}

#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
// #[non_exhaustive]
pub enum Operation {
	Nop = 0,
	Readv = 1,
	Writev = 2,
	Fsync = 3,
	ReadFixed = 4,
	WriteFixed = 5,
	PollAdd = 6,
	PollRemove = 7,
}

impl Default for Operation {
	fn default() -> Self {
		Operation::Nop
	}
}

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct RawOperation(pub u8);
impl RawOperation {
	pub fn decode(self) -> Option<Operation> {
		Some(match self.0 {
			0 => Operation::Nop,
			1 => Operation::Readv,
			2 => Operation::Writev,
			3 => Operation::Fsync,
			4 => Operation::ReadFixed,
			5 => Operation::WriteFixed,
			6 => Operation::PollAdd,
			7 => Operation::PollRemove,
			_ => return None,
		})
	}
}

impl From<Operation> for RawOperation {
	fn from(op: Operation) -> Self {
		RawOperation(op as u8)
	}
}

impl fmt::Debug for RawOperation {
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		match self.decode() {
			Some(op) => op.fmt(f),
			None => f.debug_tuple("RawOperation").field(&self.0).finish(),
		}
	}
}

bitflags! {
	#[derive(Default)]
	pub struct SubmissionEntryFlags: u8 {
		/// IOSQE_FIXED_FILE: use fixed fileset
		///
		/// I.e. `SubmissionEntry.fd` is used as index into the
		/// registered fileset (array of fds) instead.
		const FIXED_FILE = (1 << 0);

		// don't truncate any bits
		#[doc(hidden)]
		const _ALL = !0;
	}
}

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct EncodedIoPriority(pub u16);

#[repr(C)]
pub union SubmissionEntryOperationFlags {
	pub raw: u32,
	pub rw_flags: ReadWriteFlags,
	pub fsync_flags: FsyncFlags,
	pub poll_events: PollFlags,
}

impl fmt::Debug for SubmissionEntryOperationFlags {
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		f.debug_struct("SubmissionEntryOperationFlags")
			.field("raw", unsafe { &self.raw })
			.field("rw_flags", unsafe { &self.rw_flags })
			.field("fsync_flags", unsafe { &self.fsync_flags })
			.field("poll_events", unsafe { &self.poll_events })
			.finish()
	}
}

impl From<u32> for SubmissionEntryOperationFlags {
	fn from(raw: u32) -> Self {
		Self { raw }
	}
}

impl From<ReadWriteFlags> for SubmissionEntryOperationFlags {
	fn from(rw_flags: ReadWriteFlags) -> Self {
		Self { rw_flags }
	}
}

impl From<FsyncFlags> for SubmissionEntryOperationFlags {
	fn from(fsync_flags: FsyncFlags) -> Self {
		Self { fsync_flags }
	}
}

impl From<PollFlags> for SubmissionEntryOperationFlags {
	fn from(poll_events: PollFlags) -> Self {
		Self { poll_events }
	}
}

bitflags! {
	#[derive(Default)]
	pub struct ReadWriteFlags: u32 {
		/// High priority read/write.  Allows block-based filesystems to
		/// use polling of the device, which provides lower latency, but
		/// may use additional resources.  (Currently, this feature is
		/// usable only  on  a  file  descriptor opened using the
		/// O_DIRECT flag.)
		///
		/// (since Linux 4.6)
		const HIPRI = 0x00000001;

		/// Provide a per-write equivalent of the O_DSYNC open(2) flag.
		/// This flag is meaningful only for pwritev2(), and its effect
		/// applies only to the data range written by the system call.
		///
		/// (since Linux 4.7)
		const DSYNC = 0x00000002;

		/// Provide a per-write equivalent of the O_SYNC open(2) flag.
		/// This flag is meaningful only for pwritev2(), and its effect
		/// applies only to the data range written by the system call.
		///
		/// (since Linux 4.7)
		const SYNC = 0x00000004;


		/// Do not wait for data which is not immediately available.  If
		/// this flag is specified, the preadv2() system call will
		/// return instantly if it would have to read data from the
		/// backing storage or wait for a lock.  If some data was
		/// successfully read, it will return the number of bytes read.
		/// If no bytes were read, it will return -1 and set errno to
		/// EAGAIN.  Currently, this flag is meaningful only for
		/// preadv2().
		///
		/// (since Linux 4.14)
		const NOWAIT = 0x00000008;

		/// Provide a per-write equivalent of the O_APPEND open(2) flag.
		/// This flag is meaningful only for pwritev2(), and its effect
		/// applies only to the data range written by the system call.
		/// The offset argument does not affect the write operation; the
		/// data is always appended to the end of the file.  However, if
		/// the offset argument is -1, the current file offset is
		/// updated.
		///
		/// (since Linux 4.16)
		const APPEND = 0x00000010;

		const SUPPORTED = 0
			| Self::HIPRI.bits
			| Self::DSYNC.bits
			| Self::SYNC.bits
			| Self::NOWAIT.bits
			| Self::APPEND.bits
		;

		// don't truncate any bits
		#[doc(hidden)]
		const _ALL = !0;
	}
}

bitflags! {
	#[derive(Default)]
	pub struct FsyncFlags: u32 {
		const DATASYNC = (1 << 0);

		// don't truncate any bits
		#[doc(hidden)]
		const _ALL = !0;
	}
}

bitflags! {
	#[derive(Default)]
	pub struct PollFlags: u16 {
		const IN = libc::POLLIN as u16;
		const OUT = libc::POLLOUT as u16;
		const PRI = libc::POLLPRI as u16;
		const ERR = libc::POLLERR as u16;
		const NVAL = libc::POLLNVAL as u16;
		const RDNORM = libc::POLLRDNORM as u16;
		const RDBAND = libc::POLLRDBAND as u16;
		const WRNORM = libc::POLLWRNORM as u16;
		const WRBAND = libc::POLLWRBAND as u16;
		const HUP = libc::POLLHUP as u16;
		const RDHUP = 0x2000; // sparc: 0x800; // TODO: libc::POLLRDHUP as u16;
		const MSG = 0x0400; // sparc: 0x200; // TODO: libc::POLLMSG as u16;

		// don't truncate any bits
		#[doc(hidden)]
		const _ALL = !0;
	}
}


#[repr(C)]
#[derive(Clone, Copy)]
pub union SubmissionEntryExtraData {
	pub fixed: SubmissionEntryFixedOp,
	_pad2: [u64; 3],
}

impl fmt::Debug for SubmissionEntryExtraData {
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		f.debug_struct("SubmissionEntryExtraData")
			.field("fixed", unsafe { &self.fixed })
			.finish()
	}
}

#[derive(Clone, Copy, Default, Debug)]
#[repr(C)]
pub struct SubmissionEntryFixedOp {
	/// index into fixed buffers
	pub buf_index: u16,
}

/// C: `struct io_uring_cqe`
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct CompletionEntry {
	pub user_data: u64,
	pub res: i32,
	pub flags: u32,
}
