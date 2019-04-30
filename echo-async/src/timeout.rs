use futures_util::compat::Compat01As03;
use tokio_timer::Delay;

use std::{
	io,
	time::Duration,
	future::Future,
	pin::Pin,
	task::{
		Context,
		Poll,
	},
};

pub enum TimeoutError<E> {
	Timeout,
	Inner(E),
	Timer(tokio_timer::Error),
}

impl<E> From<TimeoutError<E>> for io::Error
where
	E: Into<io::Error>,
{
	fn from(e: TimeoutError<E>) -> io::Error {
		match e {
			TimeoutError::Timeout => io::Error::new(io::ErrorKind::TimedOut, "async operation timed out"),
			TimeoutError::Inner(e) => e.into(),
			TimeoutError::Timer(e) => io::Error::new(io::ErrorKind::Other, e),
		}
	}
}

pub struct Timeout<T> {
	timeout: Duration,
	delay: Option<Compat01As03<Delay>>,
	inner: T,
}

impl<T> Timeout<T> {
	pub fn new(inner: T, timeout: Duration) -> Self {
		Timeout {
			timeout,
			delay: None,
			inner,
		}
	}
}

impl<I, E, T> Future for Timeout<T>
where
	T: Future<Output = Result<I, E>>,
{
	type Output = Result<I, TimeoutError<E>>;

	fn poll(self: Pin<&mut Self>, ctx: &mut Context<'_>) -> Poll<Self::Output> {
		// must not move this.inner or this.delay
		let this = unsafe { self.get_unchecked_mut() };
		match unsafe { Pin::new_unchecked(&mut this.inner) }.poll(ctx) {
			Poll::Pending => {
				if this.delay.is_none() {
					let now = tokio_timer::clock::now();
					this.delay = Some(Compat01As03::new(Delay::new(now + this.timeout)));
				}
				match unsafe { Pin::new_unchecked(&mut this.delay.as_mut().expect("delay")) }.poll(ctx) {
					Poll::Pending => Poll::Pending,
					Poll::Ready(Ok(())) => Poll::Ready(Err(TimeoutError::Timeout)),
					Poll::Ready(Err(e)) => Poll::Ready(Err(TimeoutError::Timer(e))),
				}
			},
			Poll::Ready(r) => {
				this.delay = None; // don't move, reset in-place
				Poll::Ready(r.map_err(TimeoutError::Inner))
			}
		}
	}
}

pub trait TryFutureExt<I, E>: Future<Output = Result<I, E>> + Sized {
	fn timeout(self, timeout: Duration) -> Timeout<Self> {
		Timeout::new(self, timeout)
	}
}

impl<I, E, T> TryFutureExt<I, E> for T
where
	T: Future<Output = Result<I, E>>
{
}
