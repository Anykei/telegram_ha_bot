# Local compatibility patch

Source: crates.io `proc-macro-error2` 2.0.1, checksum
`11ec05c52be0a07b08061f7dd003e7d7092e0472bc731b4af7bb1ef876109802`.
Upstream: https://github.com/GnomedDev/proc-macro-error-2

The only source change is `extern crate proc_macro;` →
`pub extern crate proc_macro;` in `src/lib.rs`. The crate publicly re-exports
this import in its macro support module. Rust reports the private import as
future-incompatible (E0365 / rust-lang/rust#127909).

Keep upstream license files and tests. No warning suppression is used.
Remove the root `[patch.crates-io]` entry and this directory when a published
upstream release includes the fix; then update Cargo.lock and run the full tests.

Validated by compiling the bot's procedural macros on Rust 1.99.1 and running
the application tests. The Docker dependency stage copies this directory before
`cargo chef cook` so the same patch is used in release builds.
