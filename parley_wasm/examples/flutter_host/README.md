# Flutter Host for `parley.wasm`

Runs the **exact same `parley.wasm` Component Model binary** used by Servo
inside a Flutter desktop/mobile app via `dart:ffi` + Wasmtime + `vello_cpu`.

## Run on Linux / macOS / Windows

```sh
# 1. Build parley.wasm (from parley_wasm/)
cd ../..
cargo build --release --target wasm32-unknown-unknown
wasm-tools component new target/wasm32-unknown-unknown/release/parley_wasm.wasm -o target/parley.wasm

# 2. Build the Rust FFI host library
cd examples/flutter_host/native
cargo build --release

# 3. Run the Flutter app
cd ..
flutter run -d linux   # or -d macos / -d windows
```
