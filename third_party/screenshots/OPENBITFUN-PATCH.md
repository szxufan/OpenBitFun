# Linux Wayland builds under the tightened never-type fallback

Source: crates.io `screenshots` 0.8.10, from
<https://github.com/nashaofu/screenshots-rs>. This copy retains the upstream
Apache-2.0 license. The published Rust sources, manifest, README and license
are retained. 0.8.10 is the crate's final release (2024-03-01; the repository
renamed itself to xcap and never tagged 0.8.10), so no fixed release can come.

Rust 1.99 tightened never-type fallback inference. The two unconstrained
`Proxy::method_call(...)` calls in `src/linux/wayland_screenshot.rs` now infer
their return type as `!`, failing `error[E0277]: the trait bound '!: ReadAll'
is not satisfied` on Linux targets. Windows and macOS do not compile this file.

Both calls specify their return type explicitly (`::<_, _, ()>`) so inference
no longer depends on the removed fallback. No behavior changes.

Removal condition: migrate the computer-use service to the maintained XCap
crate, or drop Linux screen capture.
