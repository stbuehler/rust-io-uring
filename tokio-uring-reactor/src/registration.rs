use std::any::Any;
use std::cell::UnsafeCell;
use std::fmt;
use std::marker::PhantomData;
use std::rc::Rc;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct UringResult {
	pub result: i32,
	pub flags: u32,
}

#[derive(Default)]
struct Inner {
	result: UringResult,
	finished: bool,
	waker: futures::task::AtomicTask,
	data: Option<Box<dyn Any>>,
}

pub struct RawRegistration {
	inner: Rc<UnsafeCell<Inner>>,
}

impl RawRegistration {
	pub fn notify(&mut self, result: UringResult) {
		let inner = unsafe { &mut *self.inner.get() };
		assert!(!inner.finished);
		inner.finished = true;
		inner.result = result;
		inner.waker.notify();
	}

	pub unsafe fn into_user_data(self) -> u64 {
		let user_data = Rc::into_raw(self.inner) as usize as u64;
		assert!(user_data != 0 && user_data & 0x1 == 0);
		user_data
	}

	pub unsafe fn from_user_data(data: u64) -> Self {
		RawRegistration {
			inner: Rc::from_raw(data as usize as *const UnsafeCell<Inner>),
		}
	}
}

pub struct Registration<T: 'static> {
	inner: Rc<UnsafeCell<Inner>>,
	_data_type: PhantomData<T>,
}

impl<T: 'static> Registration<T> {
	pub fn new(data: T) -> Self {
		let inner = Inner {
			data: Some(Box::new(data)),
			.. Inner::default()
		};
		Registration {
			inner: Rc::new(UnsafeCell::new(inner)),
			_data_type: PhantomData,
		}
	}

	pub fn track(&mut self) {
		let inner = unsafe { &mut *self.inner.get() };
		inner.waker.register();
	}

	pub fn poll(&mut self) -> futures::Async<(UringResult, T)> {
		let inner = unsafe { &mut *self.inner.get() };
		if inner.data.is_none() {
			// or panic? can't become ready again
			return futures::Async::NotReady;
		}
		if inner.finished {
			let result = inner.result;
			let data = inner.data.take().expect("data").downcast::<T>().expect("type");
			futures::Async::Ready((result, *data))
		} else {
			inner.waker.register();
			futures::Async::NotReady
		}
	}

	pub fn abort(self) -> Option<T> {
		Some(*Rc::try_unwrap(self.inner).ok()?.into_inner().data.expect("data").downcast::<T>().expect("type"))
	}

	pub fn user_data(&self) -> u64 {
		let user_data = &(*self.inner) as *const UnsafeCell<Inner> as usize as u64;
		assert!(user_data != 0 && user_data & 0x1 == 0);
		user_data
	}

	pub fn to_raw(&self) -> RawRegistration {
		let inner = self.inner.clone();
		RawRegistration {
			inner,
		}
	}

	pub unsafe fn data_mut(&mut self) -> &mut T {
		let inner = &mut *self.inner.get();
		inner.data.as_mut().expect("data").downcast_mut::<T>().expect("type")
	}
}

impl Registration<()> {
	// if there is no data we can easily reuse the registration; the
	// caller must track though whether the registration is active or
	// not.
	pub fn poll_stream_and_reset(&mut self) -> futures::Async<UringResult> {
		let inner = unsafe { &mut *self.inner.get() };
		if inner.finished {
			inner.finished = false; // reset
			futures::Async::Ready(inner.result)
		} else {
			inner.waker.register();
			futures::Async::NotReady
		}
	}
}

impl<T> fmt::Debug for Registration<T> {
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		let inner = unsafe { &mut *self.inner.get() };

		let user_data = self.user_data();

		f.debug_struct("Registration")
			.field("user_data", &user_data)
			.field("finished", &inner.finished)
			.field("result", &inner.result)
			.finish()
	}
}
