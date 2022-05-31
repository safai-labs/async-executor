# Yielding Async Executor for WebAssembly

[![Crate](https://img.shields.io/crates/v/yielding-async-executor.svg)](https://crates.io/crates/yielding-async-executor)
[![API](https://docs.rs/yielding-async-executor/badge.svg)](https://docs.rs/yielding-async-executor)

This executor is a fork of [wasm-rs-async-executor](https://github.com/wasm-rs/async-executor) decoupled from wasm-bindgen.

There are a number of async task executors available in Rust's ecosystem.
However, most (if not all?) of them rely on primitives that might not be
available or optimal for WebAssembly deployment at the time and rely on the
ability to monopolize a thread until completion. This crate gives you a simple
executor that you can configure to yield in arbitrary ways.

## Usage

Include this dependency in your `Cargo.toml`:

```toml
[dependencies]
yielding-async-executor = "0.9.0"
```

`yielding-async-executor` is expected to work on stable Rust, 1.49.0 and higher up.

## Supported targets

This crate passes tests for `wasm32-unknown-unknown` and `wasm32-wasi` targets and should not really be used for anything else. There are better executors for other targets.

## Notes

Please note that this library hasn't received much analysis in terms of safety
and soundness. Some of the caveats related to that might never be resolved
completely. This is an ongoing development and the maintainer is aware of
potential pitfalls. Any productive reports of unsafeties or unsoundness are
welcomed (whether they can be resolved or simply walled with `unsafe` for end-user
to note).

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT) at your option.
