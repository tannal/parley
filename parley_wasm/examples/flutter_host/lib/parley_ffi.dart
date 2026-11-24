import 'dart:convert';
import 'dart:ffi';
import 'dart:io';
import 'dart:typed_data';

typedef _NewFromBytesNative = Pointer<Void> Function(
    Pointer<Uint8>, IntPtr, Pointer<Uint8>, Float, Float);
typedef _NewFromBytesDart = Pointer<Void> Function(
    Pointer<Uint8>, int, Pointer<Uint8>, double, double);

typedef _FreeNative = Void Function(Pointer<Void>);
typedef _FreeDart = void Function(Pointer<Void>);

typedef _StrNative = Bool Function(Pointer<Void>, Pointer<Uint8>);
typedef _StrDart = bool Function(Pointer<Void>, Pointer<Uint8>);

typedef _ActionNative = Bool Function(Pointer<Void>, Uint32, Bool);
typedef _ActionDart = bool Function(Pointer<Void>, int, bool);

typedef _PointerNative = Bool Function(
    Pointer<Void>, Uint32, Float, Float, Uint32, Bool);
typedef _PointerDart = bool Function(
    Pointer<Void>, int, double, double, int, bool);

typedef _ScrollNative = Bool Function(Pointer<Void>, Float);
typedef _ScrollDart = bool Function(Pointer<Void>, double);

typedef _ResizeNative = Bool Function(Pointer<Void>, Float, Float);
typedef _ResizeDart = bool Function(Pointer<Void>, double, double);

typedef _FramePtrNative = Pointer<Uint8> Function(Pointer<Void>);
typedef _FramePtrDart = Pointer<Uint8> Function(Pointer<Void>);

typedef _DimNative = Uint32 Function(Pointer<Void>);
typedef _DimDart = int Function(Pointer<Void>);

/// Thin `dart:ffi` wrapper around `libparley_flutter_host`, which embeds
/// Wasmtime + `vello_cpu` and executes the exact same `parley.wasm` component
/// used by Servo. Works on macOS, iOS (static lib + Pulley), Linux, Windows,
/// and Android.
class ParleyWasmBridge {
  late final DynamicLibrary _lib;
  late final _NewFromBytesDart _newFromBytes;
  late final _FreeDart _free;
  late final _StrDart _insertText;
  late final _StrDart _composeUpdate;
  late final _StrDart _composeCommit;
  late final _ActionDart _action;
  late final _PointerDart _pointer;
  late final _ScrollDart _scroll;
  late final _ResizeDart _resize;
  late final _FramePtrDart _framePtr;
  late final _DimDart _width;
  late final _DimDart _height;

  Pointer<Void> _handle = nullptr;

  ParleyWasmBridge.fromBytes({
    required Uint8List wasmBytes,
    required String initialText,
    required double width,
    required double height,
    String? libPath,
  }) {
    _lib = _openLibrary(libPath);
    _newFromBytes = _lib.lookupFunction<_NewFromBytesNative, _NewFromBytesDart>(
        'parley_host_new_from_bytes');
    _free = _lib.lookupFunction<_FreeNative, _FreeDart>('parley_host_free');
    _insertText =
        _lib.lookupFunction<_StrNative, _StrDart>('parley_host_insert_text');
    _composeUpdate = _lib
        .lookupFunction<_StrNative, _StrDart>('parley_host_compose_update');
    _composeCommit = _lib
        .lookupFunction<_StrNative, _StrDart>('parley_host_compose_commit');
    _action =
        _lib.lookupFunction<_ActionNative, _ActionDart>('parley_host_action');
    _pointer = _lib
        .lookupFunction<_PointerNative, _PointerDart>('parley_host_pointer');
    _scroll =
        _lib.lookupFunction<_ScrollNative, _ScrollDart>('parley_host_scroll');
    _resize =
        _lib.lookupFunction<_ResizeNative, _ResizeDart>('parley_host_resize');
    _framePtr = _lib
        .lookupFunction<_FramePtrNative, _FramePtrDart>('parley_host_frame_ptr');
    _width = _lib.lookupFunction<_DimNative, _DimDart>('parley_host_width');
    _height = _lib.lookupFunction<_DimNative, _DimDart>('parley_host_height');

    final wasmPtr = _allocBytes(wasmBytes);
    final initC = _allocUtf8(initialText);
    try {
      _handle = _newFromBytes(wasmPtr, wasmBytes.length, initC, width, height);
    } finally {
      _freeBytes(wasmPtr);
      _freeBytes(initC);
    }
    if (_handle == nullptr) {
      throw StateError('Failed to instantiate parley.wasm component bytes');
    }
  }

  static DynamicLibrary _openLibrary(String? overridePath) {
    if (overridePath != null && overridePath.isNotEmpty) {
      return DynamicLibrary.open(overridePath);
    }
    if (Platform.isIOS) {
      // On iOS, `libparley_flutter_host.a` is statically linked into Runner.
      return DynamicLibrary.process();
    }
    final envPath = Platform.environment['PARLEY_HOST_LIB'];
    if (envPath != null && envPath.isNotEmpty) {
      return DynamicLibrary.open(envPath);
    }

    final candidates = <String>[
      if (Platform.isMacOS) ...[
        'libparley_flutter_host.dylib',
        '${File(Platform.resolvedExecutable).parent.path}/../Frameworks/libparley_flutter_host.dylib',
        'native/target/release/libparley_flutter_host.dylib',
      ] else if (Platform.isLinux) ...[
        'libparley_flutter_host.so',
        'native/target/release/libparley_flutter_host.so',
      ] else if (Platform.isAndroid) ...[
        'libparley_flutter_host.so',
      ] else if (Platform.isWindows) ...[
        'parley_flutter_host.dll',
        'native/target/release/parley_flutter_host.dll',
      ],
    ];

    Object? lastError;
    for (final path in candidates) {
      try {
        return DynamicLibrary.open(path);
      } catch (e) {
        lastError = e;
      }
    }
    throw StateError(
      'Could not load libparley_flutter_host (tried: ${candidates.join(', ')}).\n'
      'Last error: $lastError',
    );
  }

  bool insertText(String text) => _callStr(_insertText, text);
  bool composeUpdate(String text) => _callStr(_composeUpdate, text);
  bool composeCommit(String text) => _callStr(_composeCommit, text);

  bool action(int code, [bool flag = false]) => _action(_handle, code, flag);

  bool pointerDown(double x, double y, {int clicks = 1, bool shift = false}) =>
      _pointer(_handle, 0, x, y, clicks, shift);
  bool pointerMove(double x, double y) => _pointer(_handle, 1, x, y, 0, false);
  bool pointerUp() => _pointer(_handle, 2, 0, 0, 0, false);

  bool scroll(double deltaY) => _scroll(_handle, deltaY);
  bool resize(double width, double height) => _resize(_handle, width, height);

  ({int width, int height, Uint8List rgba}) snapshotFrame() {
    final w = _width(_handle);
    final h = _height(_handle);
    final ptr = _framePtr(_handle);
    final len = w * h * 4;
    if (w == 0 || h == 0 || ptr == nullptr) {
      return (width: 0, height: 0, rgba: Uint8List(0));
    }
    return (
      width: w,
      height: h,
      rgba: Uint8List.fromList(ptr.asTypedList(len)),
    );
  }

  void dispose() {
    if (_handle != nullptr) {
      _free(_handle);
      _handle = nullptr;
    }
  }

  bool _callStr(_StrDart fn, String s) {
    final ptr = _allocUtf8(s);
    try {
      return fn(_handle, ptr);
    } finally {
      _freeBytes(ptr);
    }
  }

  static final _malloc = DynamicLibrary.process().lookupFunction<
      Pointer<Uint8> Function(IntPtr),
      Pointer<Uint8> Function(int)>('malloc');
  static final _libcFree = DynamicLibrary.process().lookupFunction<
      Void Function(Pointer<Uint8>),
      void Function(Pointer<Uint8>)>('free');

  static Pointer<Uint8> _allocBytes(Uint8List bytes) {
    final ptr = _malloc(bytes.length);
    ptr.asTypedList(bytes.length).setAll(0, bytes);
    return ptr;
  }

  static Pointer<Uint8> _allocUtf8(String s) {
    final bytes = utf8.encode(s);
    final ptr = _malloc(bytes.length + 1);
    final view = ptr.asTypedList(bytes.length + 1);
    view.setAll(0, bytes);
    view[bytes.length] = 0;
    return ptr;
  }

  static void _freeBytes(Pointer<Uint8> ptr) => _libcFree(ptr);
}
