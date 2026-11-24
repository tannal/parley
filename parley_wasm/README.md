# `parley_wasm` — WebAssembly Component Model Text Editor

Compiles Parley (`default-features = false, features = ["std"]`, no `fontique/system`)
to `wasm32-unknown-unknown` and wraps it as a WebAssembly Component implementing
`servo:parley-editor@0.1.0`.

## Build `parley.wasm`

```sh
rustup target add wasm32-unknown-unknown
cargo build --release --target wasm32-unknown-unknown
wasm-tools component new \
  target/wasm32-unknown-unknown/release/parley_wasm.wasm \
  -o target/parley.wasm
```

## Examples (Multiple Hosts, One `parley.wasm` Binary)

- **Servo Browser** (`tannal/servo`, branch `wasm-parley`): Loads `<script src="parley.wasm">` directly, bridging Servo's `FontContext`, Vello Canvas 2D backend, and OS IME.
- **[`examples/standalone_host`](examples/standalone_host/)**: Zero-browser, zero-DOM Rust host (Wasmtime + `vello_cpu`) that renders `parley.wasm` (including CJK + inline IME preedit) directly to a PNG.
- **[`examples/flutter_host`](examples/flutter_host/)**: Flutter desktop/mobile app running the exact same `parley.wasm` binary via `dart:ffi` + Wasmtime + `vello_cpu`.
