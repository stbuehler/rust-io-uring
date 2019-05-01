use futures_util;
use futures_util::{
	compat::Compat,
	try_stream::TryStreamExt as _,
	try_future::TryFutureExt as _,
	future::FutureExt as _,
};
use std::future::Future;
use std::io;
use std::net;
use std::time::Duration;
use std::rc::Rc;
use tokio_uring_reactor::{
	Handle,
};
use crate::timeout::{TryFutureExt as _};

pub fn serve<H, HF, HE, HEF>(runtime: &mut tokio_uring::Runtime, l: net::TcpListener, handle_con: H, handle_err: HE)
where
	H: Fn(Handle, tokio_uring_reactor::net::TcpStream, net::SocketAddr) -> HF + 'static,
	HF: Future<Output = io::Result<()>> + 'static,
	HE: Fn(Handle, io::Error, net::SocketAddr) -> HEF + 'static,
	HEF: Future<Output = ()> + 'static,
{
	let handle = runtime.reactor_handle();
	runtime.spawn(Compat::new(Box::pin(async move {
		let handle_err = Rc::new(handle_err);

		if let Err::<(), io::Error>(e) = await!(async {
			let l = tokio_uring_reactor::net::TcpListener::from(l);
			let mut i = l.incoming(&handle);

			loop {
				let (con, addr) = await!(i.try_next().timeout(Duration::from_secs(30)))?.unwrap();
				let ehandle = handle.clone();
				let handle_err = handle_err.clone();
				tokio_current_thread::spawn(Compat::new(Box::pin(
					handle_con(handle.clone(), con, addr.clone())
					.or_else(move |e| handle_err(ehandle, e, addr).map(Ok))
				)))
			}
		}) {
			eprintln!("Serve error: {}", e);
		}

		Ok::<(), ()>(())
	})));
}
