# `parley_wasm` — WebAssembly Component Model Text Editor for Servo

Compiles Parley (`default-features = false, features = ["std"]`, no `fontique/system`)
to `wasm32-unknown-unknown` and wraps it as a WebAssembly Component implementing
`servo:parley-editor@0.1.0`.

## Build

```sh
rustup target add wasm32-unknown-unknown
cargo build --release --target wasm32-unknown-unknown
wasm-tools component new \
  target/wasm32-unknown-unknown/release/parley_wasm.wasm \
  -o target/parley.wasm
```

## Architecture

- **Fonts (`servo:parley-editor/fonts`)**: Resolved by Servo's `FontContext`
  (system + `@font-face`). Font bytes are loaded once per `font-id` into Wasm
  linear memory for HarfRust shaping.
- **Rendering (`servo:parley-editor/surface`)**: Emits batched selection/caret
  rects and positioned glyph runs straight to Servo's Canvas 2D / Vello backend
  using `CanvasCommand::ProcessBatchMessages`.
- **Input & IME (`servo:parley-editor/editing`)**: Keyboard shortcuts, pointer
  clicks/drags, IME composition, and clipboard are translated by Servo and
  handled by `parley::PlainEditor`.
