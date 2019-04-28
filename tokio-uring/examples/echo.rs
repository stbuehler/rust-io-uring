use std::io;
use std::net;
use std::time::Duration;
use futures::prelude::*;
use tokio_uring_reactor::io::{
	SocketRead,
	SocketWrite,
};
use tokio_timer::Timeout;

pub fn main() {
	env_logger::init();

	println!("Starting echo server");

	let l = net::TcpListener::bind("[::]:22").expect("bind");
	let l = tokio_uring_reactor::net::TcpListener::from(l);

	let mut runtime = tokio_uring::Runtime::new().expect("new runtime");

	let handle = runtime.reactor_handle();
	let connection_handler = move |(c, a): (tokio_uring_reactor::net::TcpStream, net::SocketAddr)| {
		println!("Connection from {}", a);
		let mut buf: Vec<u8> = Vec::new();
		let whandle = handle.clone();
		buf.resize_with(512, Default::default);
		tokio_current_thread::spawn(
			Timeout::new(c.read(&handle, buf).from_err(), Duration::from_secs(3))
			.map_err(|e| {
				eprintln!("timout/read error");
				if e.is_inner() {
					e.into_inner().expect("inner")
				} else {
					io::Error::new(io::ErrorKind::TimedOut, "timeout")
				}
			})
			.and_then(move |(n, mut buf, c)| {
				buf.truncate(n);
				println!("Echoing: {:?}", buf);
				c.write(&whandle, buf).from_err()
			})
			.map(|(_,_,_)| println!("connection done"))
			.map_err(|e| eprintln!("Connection error: {}", e))
		);
		Ok(())
	};

	let handle = runtime.reactor_handle();
	runtime.spawn(
		Timeout::new(l.incoming(&handle), Duration::from_secs(30))
		.map_err(|e| {
			if e.is_inner() {
				panic!(e.into_inner().expect("inner"));
			}
		})
		.for_each(connection_handler)
		.map(|()| eprintln!("listening done"))
	);
	runtime.run().expect("runtime run");
}
