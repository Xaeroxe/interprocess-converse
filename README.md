# interprocess-converse

![ci status](https://github.com/Xaeroxe/interprocess-converse/actions/workflows/rust.yml/badge.svg)

A wrapper over the [`interprocess-typed`](https://github.com/Xaeroxe/interprocess-typed) crate which allows
[`serde`](https://github.com/serde-rs/serde) compatible types to be sent over a [`tokio`](https://github.com/tokio-rs/tokio)
local socket with async. `interprocess-converse` adds the ability to receive replies from the other process.
`interprocess-typed` is itself a wrapper over the [`interprocess`](https://github.com/kotauskas/interprocess) crate,
which provides a raw byte stream between processes.

`interprocess-converse` depends on `interprocess-typed`, `interprocess-typed` depends on `interprocess`.

## Who needs this?

Anyone who wishes to send messages between two processes running on the same system, and get replies to those messages.
This crate supports all the same systems as `interprocess`, which at time of writing includes

- Windows
- MacOS
- Linux
- Other Unix platforms such as BSDs, illumos, and Solaris

Unlike similar offerings in the Rust ecosystem, this crate supports having multiple clients connected to one 
server, and so is appropriate for use in daemons.

## Why shouldn't I just use `interprocess` (or `interprocess-typed`) directly?

It depends on what you want to send! `interprocess` transmits raw streams of bytes, with no message delimiters or 
format. `interprocess-typed` allows you to send Rust types instead. Specifically, types that are serde-compatible.
`interprocess-converse` then adds the ability to receive replies to your typed messages.
If your data is more easily described as a stream of bytes then you should use the `interprocess` crate directly. 
However, if you'd rather work with complex types, consider either `interprocess-typed` or `interprocess-converse`.

## Alternative async runtimes

`interprocess-converse` only supports `tokio` and has no plans to support alternate `async` runtimes.

## Contributing

Contributions are welcome! Please ensure your changes to the code pass unit tests. If you're fixing a bug please
add a unit test so that someone doesn't un-fix the bug later.