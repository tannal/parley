import 'dart:async';
import 'dart:ui' as ui;

import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import 'parley_ffi.dart';

void main() {
  runApp(const ParleyFlutterApp());
}

class ParleyFlutterApp extends StatelessWidget {
  const ParleyFlutterApp({super.key});

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'Parley.wasm in Flutter',
      debugShowCheckedModeBanner: false,
      theme: ThemeData.dark(useMaterial3: true),
      home: const Scaffold(
        backgroundColor: Color(0xFF0F172A),
        body: Center(
          child: SizedBox(
            width: 960,
            height: 640,
            child: ParleyEditorWidget(),
          ),
        ),
      ),
    );
  }
}

class ParleyEditorWidget extends StatefulWidget {
  const ParleyEditorWidget({super.key});

  @override
  State<ParleyEditorWidget> createState() => _ParleyEditorWidgetState();
}

class _ParleyEditorWidgetState extends State<ParleyEditorWidget> {
  final FocusNode _focusNode = FocusNode();
  ParleyWasmBridge? _bridge;
  ui.Image? _frameImage;
  Timer? _blinkTimer;
  bool _cursorVisible = true;
  String? _error;

  @override
  void initState() {
    super.initState();
    try {
      _bridge = ParleyWasmBridge(
        libPath: ParleyWasmBridge.defaultLibPath(),
        wasmPath: '../../target/parley.wasm',
        initialText: 'Parley.wasm Running Inside Flutter!\n\n'
            'The EXACT SAME wasm32-unknown-unknown Component Model binary '
            'from Servo is rendering here via dart:ffi + Wasmtime + vello_cpu.\n\n'
            '你好，世界！Click, drag to select, type, or use Ctrl+Z / Ctrl+Shift+Z.',
        width: 960,
        height: 640,
      );
      _refreshFrame();
      _blinkTimer = Timer.periodic(const Duration(milliseconds: 530), (_) {
        if (_bridge == null || !_focusNode.hasFocus) return;
        _cursorVisible = !_cursorVisible;
        if (_bridge!.action(12, _cursorVisible)) {
          _refreshFrame();
        }
      });
    } catch (e) {
      _error = '$e\n\nBuild the Rust FFI host first:\n'
          '  cd native && cargo build --release';
    }
  }

  void _refreshFrame() {
    final bridge = _bridge;
    if (bridge == null) return;
    final snap = bridge.snapshotFrame();
    if (snap.width == 0 || snap.height == 0) return;
    ui.decodeImageFromPixels(
      snap.rgba,
      snap.width,
      snap.height,
      ui.PixelFormat.rgba8888,
      (image) {
        if (!mounted) {
          image.dispose();
          return;
        }
        final old = _frameImage;
        setState(() => _frameImage = image);
        old?.dispose();
      },
    );
  }

  void _handleKey(KeyEvent event) {
    final bridge = _bridge;
    if (bridge == null || event is KeyUpEvent) return;

    final shift = HardwareKeyboard.instance.isShiftPressed;
    final ctrl = HardwareKeyboard.instance.isControlPressed ||
        HardwareKeyboard.instance.isMetaPressed;

    bool dirty = false;
    final key = event.logicalKey;
    if (ctrl && key == LogicalKeyboardKey.keyA) {
      dirty = bridge.action(9);
    } else if (ctrl && key == LogicalKeyboardKey.keyZ) {
      dirty = bridge.action(shift ? 11 : 10);
    } else if (ctrl && key == LogicalKeyboardKey.keyY) {
      dirty = bridge.action(11);
    } else if (key == LogicalKeyboardKey.backspace) {
      dirty = bridge.action(1);
    } else if (key == LogicalKeyboardKey.delete) {
      dirty = bridge.action(2);
    } else if (key == LogicalKeyboardKey.enter) {
      dirty = bridge.insertText('\n');
    } else if (key == LogicalKeyboardKey.arrowLeft) {
      dirty = bridge.action(3, shift);
    } else if (key == LogicalKeyboardKey.arrowRight) {
      dirty = bridge.action(4, shift);
    } else if (key == LogicalKeyboardKey.arrowUp) {
      dirty = bridge.action(5, shift);
    } else if (key == LogicalKeyboardKey.arrowDown) {
      dirty = bridge.action(6, shift);
    } else if (key == LogicalKeyboardKey.home) {
      dirty = bridge.action(7, shift);
    } else if (key == LogicalKeyboardKey.end) {
      dirty = bridge.action(8, shift);
    } else if (!ctrl && event.character != null && event.character!.isNotEmpty) {
      dirty = bridge.insertText(event.character!);
    }

    if (dirty) {
      _cursorVisible = true;
      _refreshFrame();
    }
  }

  @override
  void dispose() {
    _blinkTimer?.cancel();
    _frameImage?.dispose();
    _bridge?.dispose();
    _focusNode.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    if (_error != null) {
      return Padding(
        padding: const EdgeInsets.all(24),
        child: SelectableText(
          _error!,
          style: const TextStyle(color: Colors.amber, fontFamily: 'monospace'),
        ),
      );
    }

    return KeyboardListener(
      focusNode: _focusNode,
      autofocus: true,
      onKeyEvent: _handleKey,
      child: Listener(
        onPointerDown: (e) {
          _focusNode.requestFocus();
          if (_bridge?.pointerDown(
                e.localPosition.dx,
                e.localPosition.dy,
                shift: HardwareKeyboard.instance.isShiftPressed,
              ) ??
              false) {
            _refreshFrame();
          }
        },
        onPointerMove: (e) {
          if (_bridge?.pointerMove(e.localPosition.dx, e.localPosition.dy) ??
              false) {
            _refreshFrame();
          }
        },
        onPointerUp: (_) => _bridge?.pointerUp(),
        onPointerSignal: (signal) {
          if (signal is PointerScrollEvent) {
            if (_bridge?.scroll(signal.scrollDelta.dy) ?? false) {
              _refreshFrame();
            }
          }
        },
        child: MouseRegion(
          cursor: SystemMouseCursors.text,
          child: Container(
            decoration: BoxDecoration(
              border: Border.all(color: const Color(0xFF334155)),
              borderRadius: BorderRadius.circular(12),
            ),
            clipBehavior: Clip.antiAlias,
            child: _frameImage == null
                ? const Center(child: CircularProgressIndicator())
                : RawImage(image: _frameImage, fit: BoxFit.fill),
          ),
        ),
      ),
    );
  }
}
