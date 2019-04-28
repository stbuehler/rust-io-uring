mod mmap;

use std::sync::atomic::{
	Ordering,
	AtomicU32,
};
use std::io;
use std::os::unix::io::{
	AsRawFd,
	FromRawFd,
	IntoRawFd,
	RawFd,
};
use std::mem::size_of;

pub use io_uring_sys::*;
use crate::mmap::MappedMemory;

pub struct Uring {
	file: UringFile,
	sq: SubmissionQueue,
	cq: CompletionQueue,
}

impl Uring {
	pub fn new(entries: u32, mut params: SetupParameters) -> io::Result<Self> {
		let file = UringFile::new(entries, &mut params)?;
		let sq = SubmissionQueue::new(&file, &params.sq_off, params.sq_entries)?;
		let cq = CompletionQueue::new(&file, &params.cq_off, params.cq_entries)?;

		Ok(Uring {
			file,
			sq,
			cq,
		})
	}

	pub fn file(&mut self) -> &mut UringFile {
		&mut self.file
	}

	pub fn submission_queue(&mut self) -> &mut SubmissionQueue {
		&mut self.sq
	}

	pub fn completion_queue(&mut self) -> &mut CompletionQueue {
		&mut self.cq
	}
}

// the purpose of the indirection (index array -> sces) is not quite
// clear; we don't need it and will keep the index simple, i.e. entry i
// (masked) in ring will point at i in sces.
pub struct SubmissionQueue {
	_mmap: MappedMemory,

	// `head` is controlled by kernel. we only need to update it if it
	// seems the ring is full.
	k_head: &'static AtomicU32,
	cached_head: u32,
	// `tail` is controlled by us; we can stage multiple entries and
	// then flush them in one update
	k_tail: &'static AtomicU32,
	local_tail: u32,

	// `ring_mask` and `ring_entries` are const, so only read them once.
	ring_mask: u32,
	// k_ring_mask: *const u32,
	// ring_entries: u32,
	// k_ring_entries: *const u32,

	// `flags`: the only flag so far is controlled by the kernel
	k_flags: &'static AtomicU32, // SubmissionQueueFlags

	// `dropped` counter of invalid submissions (out-of-bound index in array)
	k_dropped: &'static AtomicU32,

	// index array; kernel only reads this, so we can init it once
	// k_array: *mut u32,

	// points to [SubmissionEntry; ring_entries]
	_mmap_entries: MappedMemory,
	sces: *mut SubmissionEntry,
}

impl SubmissionQueue {
	fn new(file: &UringFile, offsets: &SubmissionQueueRingOffsets, sq_entries: u32) -> io::Result<Self> {
		let mmap = MappedMemory::map(
			file.as_raw_fd(),
			SetupParameters::SUBMISSION_QUEUE_RING_OFFSET,
			(offsets.array as usize) + size_of::<u32>() * (sq_entries as usize),
		)?;
		let mmap_entries = MappedMemory::map(
			file.as_raw_fd(),
			SetupParameters::SUBMISSION_QUEUE_ENTRIES_OFFSET,
			size_of::<SubmissionEntry>() * (sq_entries as usize),
		)?;
		let k_head: &AtomicU32 = unsafe { &*mmap.get_field(offsets.head) };
		let cached_head = k_head.load(Ordering::Relaxed);
		let k_tail: &AtomicU32 = unsafe { &*mmap.get_field(offsets.tail) };
		let local_tail = k_tail.load(Ordering::Relaxed);
		let k_ring_mask: *mut u32 = mmap.get_field(offsets.ring_mask);
		let ring_mask = unsafe { *k_ring_mask };
		let k_ring_entries: *mut u32 = mmap.get_field(offsets.ring_entries);
		let ring_entries = unsafe { *k_ring_entries };
		let k_flags: &AtomicU32 = unsafe { &*mmap.get_field(offsets.flags) };
		let k_dropped: &AtomicU32 = unsafe { &*mmap.get_field(offsets.dropped) };
		let k_array: *mut u32 = mmap.get_field(offsets.array);
		let sces: *mut SubmissionEntry = mmap_entries.as_mut_ptr() as *mut SubmissionEntry;

		assert_eq!(sq_entries, ring_entries);
		assert!(ring_entries.is_power_of_two());
		assert_eq!(ring_mask, ring_entries - 1);

		// initialize index array to identity map: i -> i.
		for i in 0..ring_entries {
			unsafe { *k_array.add(i as usize) = i };
		}

		Ok(SubmissionQueue {
			_mmap: mmap,
			k_head,
			cached_head,
			k_tail,
			local_tail,
			ring_mask,
			// ring_entries,
			k_flags,
			k_dropped,
			_mmap_entries: mmap_entries,
			sces,
		})
	}

	fn flush_tail(&mut self) {
		log::trace!("SQ updating tail: {}", self.local_tail);
		self.k_tail.store(self.local_tail, Ordering::Release);
	}

	fn head(&mut self) -> u32 {
		if self.cached_head != self.local_tail {
			// ring not empty
			if 0 == ((self.cached_head ^ self.local_tail) & self.ring_mask) {
				// head and tail point to same entry, potentially full; refresh cache
				self.refresh_head();
			}
		}
		self.cached_head
	}

	fn refresh_head(&mut self) -> u32 {
		self.cached_head = self.k_head.load(Ordering::Acquire);
		log::trace!("Refreshed SQ head: {}", self.cached_head);
		self.cached_head
	}

	pub fn is_full(&mut self) -> bool {
		let head = self.head();
		(head != self.local_tail) // not empty
		&& 0 == ((self.cached_head ^ self.local_tail) & self.ring_mask) // point to same entry
	}

	pub fn bulk(&mut self) -> BulkSubmission {
		BulkSubmission(self)
	}

	pub fn flags(&self) -> SubmissionQueueFlags {
		SubmissionQueueFlags::from_bits_truncate(self.k_flags.load(Ordering::Relaxed))
	}

	pub fn dropped(&self) -> u32 {
		self.k_dropped.load(Ordering::Relaxed)
	}

	pub fn has_pending_submissions(&mut self) -> bool {
		self.refresh_head() != self.local_tail
	}

	pub fn pending_submissions(&mut self) -> u32 {
		self.local_tail - self.refresh_head()
	}
}

#[derive(Debug)]
pub enum SubmissionError<E> {
	QueueFull,
	FillError(E),
}

impl<E: std::fmt::Display> std::fmt::Display for SubmissionError<E> {
	fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
		match self {
			SubmissionError::QueueFull => write!(f, "submission error: queue full"),
			SubmissionError::FillError(e) => write!(f, "submission error: {}", e),
		}
	}
}

impl<E: std::error::Error + 'static> std::error::Error for SubmissionError<E> {
	fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
		match self {
			SubmissionError::QueueFull => None,
			SubmissionError::FillError(e) => Some(e),
		}
	}
}

pub struct BulkSubmission<'a>(&'a mut SubmissionQueue);

impl BulkSubmission<'_> {
	pub fn is_full(&mut self) -> bool {
		self.0.is_full()
	}

	pub fn submit_with<F, E>(&mut self, f: F) -> Result<(), SubmissionError<E>>
	where
		F: FnOnce(&mut SubmissionEntry) -> Result<(), E>
	{
		if self.is_full() { return Err(SubmissionError::QueueFull); }
		let ndx = self.0.local_tail;
		let entry = unsafe { &mut *self.0.sces.add((ndx & self.0.ring_mask) as usize) };
		entry.clear();
		f(entry).map_err(SubmissionError::FillError)?;
		log::debug!("Submit: @{} -> {:?}", ndx, entry);
		self.0.local_tail = ndx.wrapping_add(1);
		Ok(())
	}
}

impl Drop for BulkSubmission<'_> {
	fn drop(&mut self) {
		self.0.flush_tail();
	}
}

pub struct CompletionQueue {
	_mmap: MappedMemory,

	// updated by userspace; increment after entry at head was read; for
	// bulk reading only increment local_head and write back alter
	k_head: &'static AtomicU32,
	local_head: u32,

	// updated by kernel; update cached value once (cached) head reaches
	// cached tail.
	k_tail: &'static AtomicU32,
	cached_tail: u32,

	// k_ring_mask: *mut u32,
	ring_mask: u32,

	// k_ring_entries: *mut u32,
	// ring_entries: u32,

	k_overflow: &'static AtomicU32,
	k_cqes: *mut CompletionEntry,
}

impl CompletionQueue {
	fn new(file: &UringFile, offsets: &CompletionQueueRingOffsets, cq_entries: u32) -> io::Result<Self> {
		let mmap = MappedMemory::map(
			file.as_raw_fd(),
			SetupParameters::COMPLETION_QUEUE_RING_OFFSET,
			(offsets.cqes as usize) + size_of::<CompletionEntry>() * (cq_entries as usize),
		)?;
		let k_head: &AtomicU32 = unsafe { &*mmap.get_field(offsets.head) };
		let local_head = k_head.load(Ordering::Relaxed);
		let k_tail: &AtomicU32 = unsafe { &*mmap.get_field(offsets.tail) };
		let cached_tail = k_tail.load(Ordering::Relaxed);
		let k_ring_mask: *mut u32 = mmap.get_field(offsets.ring_mask);
		let ring_mask = unsafe { *k_ring_mask };
		let k_ring_entries: *mut u32 = mmap.get_field(offsets.ring_entries);
		let ring_entries = unsafe { *k_ring_entries };
		let k_overflow: &AtomicU32 = unsafe { &*mmap.get_field(offsets.overflow) };
		let k_cqes: *mut CompletionEntry = mmap.get_field(offsets.cqes);

		assert_eq!(cq_entries, ring_entries);
		assert!(ring_entries.is_power_of_two());
		assert_eq!(ring_mask, ring_entries - 1);

		Ok(CompletionQueue {
			_mmap: mmap,
			k_head,
			local_head,
			k_tail,
			cached_tail,
			ring_mask,
			// ring_entries,
			k_overflow,
			k_cqes,
		})
	}

	fn flush_head(&mut self) {
		self.k_head.store(self.local_head, Ordering::Release);
	}

	fn refresh_tail(&mut self) -> u32 {
		self.cached_tail = self.k_tail.load(Ordering::Acquire);
		self.cached_tail
	}

	fn is_empty(&mut self) -> bool {
		if self.cached_tail == self.local_head {
			self.refresh_tail();
			self.cached_tail == self.local_head
		} else {
			false
		}
	}

	pub fn overflow(&mut self) -> u32 {
		self.k_overflow.load(Ordering::Relaxed)
	}
}

impl<'a> IntoIterator for &'a mut CompletionQueue {
	type Item = CompletionEntry;
	type IntoIter = BulkCompletion<'a>;

	fn into_iter(self) -> Self::IntoIter {
		BulkCompletion(self)
	}
}

pub struct BulkCompletion<'a>(&'a mut CompletionQueue);

impl Iterator for BulkCompletion<'_> {
	type Item = CompletionEntry;

	fn next(&mut self) -> Option<Self::Item> {
		if self.0.is_empty() { return None; }
		let ndx = self.0.local_head;
		let item = unsafe { &*self.0.k_cqes.add((ndx & self.0.ring_mask) as usize) }.clone();
		self.0.local_head = ndx.wrapping_add(1);
		log::debug!("Completed: @{} -> {:?}", ndx, item);
		Some(item)
	}
}

impl Drop for BulkCompletion<'_> {
	fn drop(&mut self) {
		self.0.flush_head();
	}
}

pub struct UringFile(std::fs::File);

impl UringFile {
	pub fn new(entries: u32, params: &mut SetupParameters) -> io::Result<Self> {
		let res = unsafe {
			io_uring_setup(entries, params)
		};
		if res < 0 {
			return Err(io::Error::last_os_error());
		}
		Ok(unsafe { Self::from_raw_fd(res) })
	}

	pub fn enter(&mut self, to_submit: u32, min_complete: u32, flags: EnterFlags, sig: Option<&libc::sigset_t>) -> io::Result<()> {
		let sig = match sig {
			Some(sig) => sig as *const _,
			None => 0 as *const _,
		};
		if unsafe { io_uring_enter(self.as_raw_fd(), to_submit, min_complete, flags.bits(), sig) } < 0 {
			Err(io::Error::last_os_error())
		} else {
			Ok(())
		}
	}

	/// can only register one list of buffers at once; needs an explicit
	/// unregister before registering the next list.
	///
	/// unsafe because it passes raw pointers in the iovecs.
	pub unsafe fn register_buffers(&mut self, buffers: &[libc::iovec]) -> io::Result<()> {
		self.register(RegisterOpCode::REGISTER_BUFFERS, buffers.as_ptr() as *const _, buffers.len() as u32)
	}

	/// fails if there are currently no buffers registered.
	pub fn unregister_buffers(&mut self) -> io::Result<()> {
		unsafe {
			self.register(RegisterOpCode::UNREGISTER_BUFFERS, 0 as *const _, 0)
		}
	}

	/// can only register one list of fds at once; needs an explicit
	/// unregister before registering the next list.
	pub fn register_files(&mut self, fds: &[RawFd]) -> io::Result<()> {
		assert!(fds.len() <= u32::max_value() as usize);
		unsafe {
			self.register(RegisterOpCode::REGISTER_FILES, fds.as_ptr() as *const _, fds.len() as u32)
		}
	}

	/// fails if there is currently no fd set registered.
	pub fn unregister_files(&mut self) -> io::Result<()> {
		unsafe {
			self.register(RegisterOpCode::UNREGISTER_FILES, 0 as *const _, 0)
		}
	}

	pub unsafe fn register(&self, opcode: RegisterOpCode, arg: *const libc::c_void, nr_args: u32) -> io::Result<()> {
		if io_uring_register(self.as_raw_fd(), opcode.0, arg, nr_args) != 0 {
			Err(io::Error::last_os_error())
		} else {
			Ok(())
		}
	}
}

impl AsRawFd for UringFile {
	fn as_raw_fd(&self) -> RawFd {
		self.0.as_raw_fd()
	}
}

impl IntoRawFd for UringFile {
	fn into_raw_fd(self) -> RawFd {
		self.0.into_raw_fd()
	}
}

impl FromRawFd for UringFile {
	unsafe fn from_raw_fd(fd: RawFd) -> Self {
		UringFile(std::fs::File::from_raw_fd(fd))
	}
}
