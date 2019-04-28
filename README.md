# Proof of Concept io-uring libraries, work in progress

See https://twitter.com/axboe/status/1114568873800491009 for what io-uring is about.

Also see ["echo-server" example](tokio-uring/examples/echo.rs).

Crates:
- [`io-uring-sys`](io-uring-sys): Low-level wrapping of the kernel API and types
- [`io-uring`](io-uring): Fancy wrapping of kernel API (especially submission and completion queue)
- [`tokio-uring-reactor`](tokio-uring-reactor): Reactor (IO handling) based on `io-uring` for tokio integration.
- [`tokio-uring`](tokio-uring): tokio (current_thread) Runtime based on `io-uring`.

Right now some very basic TCP (accept, read, write) operations are supported by the tokio integration directly, but you can do almost anything using the `async_*` functions provided by the reactor handle.
