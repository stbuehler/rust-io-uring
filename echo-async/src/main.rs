#![feature(async_await, await_macro)]

pub mod timeout;

use futures_util;
use futures_util::{
	compat::Compat,
	try_stream::TryStreamExt as _,
};
use std::io;
use std::net;
use std::time::Duration;
use tokio_uring_reactor::{
	io::{
		SocketRead,
		SocketWrite,
	},
	Handle,
};
use crate::timeout::{TryFutureExt as _};

async fn handle_connection(handle: Handle, c: tokio_uring_reactor::net::TcpStream, a: net::SocketAddr) -> io::Result<()> {
	println!("Connection from {}", a);

	let mut storage: Option<Vec<u8>> = None;
	let mut connection = Some(c);

	loop {
		let mut buf = storage.take().unwrap_or_default();
		let con = connection.take().expect("connection missing");
		buf.resize_with(512, Default::default);

		// TODO: add 3sec timeout
		let (n, mut buf, con) = await!(con.read(&handle, buf).timeout(Duration::from_secs(3)))?;
		if n == 0 {
			println!("Connection from {} closing", a);
			return Ok(())
		}
		buf.truncate(n);
		println!("Echoing: {:?}", buf);
		let (_n, buf, con) = await!(con.write(&handle, buf))?;

		// put values back for next round
		storage = Some(buf);
		connection = Some(con);
	}
}

async fn handle_connection_outer(handle: Handle, c: tokio_uring_reactor::net::TcpStream, a: net::SocketAddr) -> Result<(), ()> {
	if let Err(e) = await!(handle_connection(handle, c, a.clone())) {
		eprintln!("Connection from {} error: {}", a, e);
	}

	Ok(())
}

async fn serve(handle: Handle, l: net::TcpListener) -> io::Result<()> {
	let l = tokio_uring_reactor::net::TcpListener::from(l);
	let mut i = l.incoming(&handle);

	loop {
		let (con, addr) = await!(i.try_next().timeout(Duration::from_secs(30)))?.unwrap();
		tokio_current_thread::spawn(Compat::new(Box::pin(
			handle_connection_outer(handle.clone(), con, addr)
		)))
	}

	// println!("listening done");
	// Ok(())
}

async fn serve_outer(handle: Handle, l: net::TcpListener) -> Result<(), ()> {
	if let Err(e) = await!(serve(handle, l)) {
		eprintln!("Serve error: {}", e);
	}

	Ok(())
}

pub fn main() {
	env_logger::init();

	println!("Starting echo server");

	let l = net::TcpListener::bind("[::]:22").expect("bind");

	let mut runtime = tokio_uring::Runtime::new().expect("new runtime");

	let handle = runtime.reactor_handle();
	runtime.spawn(Compat::new(Box::pin(serve_outer(handle, l))));
	runtime.run().expect("runtime run");
}
