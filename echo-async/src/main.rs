#![feature(async_await, await_macro)]

pub mod timeout;
pub mod serve_tcp;

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

pub fn main() {
	env_logger::init();

	println!("Starting echo server");

	let l = net::TcpListener::bind("[::]:22").expect("bind");

	let mut runtime = tokio_uring::Runtime::new().expect("new runtime");

	serve_tcp::serve(&mut runtime, l, handle_connection, async move |_, e, addr| {
		eprintln!("Connection from {} error: {}", addr, e);
	});
	runtime.run().expect("runtime run");
}
