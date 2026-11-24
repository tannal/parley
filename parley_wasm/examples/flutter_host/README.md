# Flutter Host for `parley.wasm` (macOS, iOS, Linux, Windows)

Runs the **exact same `parley.wasm` Component Model binary** used by Servo
inside a Flutter app via `dart:ffi` + Wasmtime + `vello_cpu`.

`assets/parley.wasm` is bundled directly in the Flutter app (no hardcoded file
paths or macOS sandbox issues).

## 1. (Optional) Rebuild `parley.wasm`

`parley_wasm/.cargo/config.toml` defaults to `wasm32-unknown-unknown`, so:

```sh
cd ../.. # parley_wasm/
rustup target add wasm32-unknown-unknown
cargo build --release
wasm-tools component new \
  target/wasm32-unknown-unknown/release/parley_wasm.wasm \
  -o examples/flutter_host/assets/parley.wasm
```

## 2. Run on macOS (Apple Silicon / Intel)

```sh
cd examples/flutter_host/native
cargo build --release
cd ..
PARLEY_HOST_LIB="$PWD/native/target/release/libparley_flutter_host.dylib" \
  flutter run -d macos
```

## 3. Run on iOS Simulator (ARM64 — Uses Wasmtime Pulley, Zero JIT)

```sh
rustup target add aarch64-apple-ios-sim
cd examples/flutter_host/native
cargo build --release --target aarch64-apple-ios-sim
```

Then link `native/target/aarch64-apple-ios-sim/release/libparley_flutter_host.a`
in `ios/Runner.xcodeproj` (Target `Runner` → Build Phases → Link Binary With
Libraries → Add Other → select `libparley_flutter_host.a`, and add
`-force_load $(SRCROOT)/../native/target/aarch64-apple-ios-sim/release/libparley_flutter_host.a`
to `Other Linker Flags` so `dart:ffi`'s `DynamicLibrary.process()` sees the
symbols), then run:

```sh
cd ..
flutter run -d ios
```
