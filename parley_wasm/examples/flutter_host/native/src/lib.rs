//! C FFI host library for Flutter (`dart:ffi`), running the exact same
//! `parley.wasm` Component Model binary via Wasmtime + `vello_cpu`.

use std::collections::HashMap;
use std::ffi::CStr;
use std::fs;
use std::os::raw::c_char;
use std::sync::Arc;

use peniko::color::{AlphaColor, Srgb};
use peniko::kurbo::Rect as KurboRect;
use peniko::{Blob, Fill, FontData};
use vello_cpu::{Glyph, Pixmap, RenderContext, Resources};
use wasmtime::component::{Component, HasSelf, Linker};
use wasmtime::{Config, Engine, Store};

wasmtime::component::bindgen!({
    world: "parley-editor",
    path: "../../../wit/parley_editor.wit",
});

use servo::parley_editor::console::Host as ConsoleHost;
use servo::parley_editor::editing::EditorEvent;
use servo::parley_editor::fonts::{FontQuery, Host as FontsHost, ResolvedFont};
use servo::parley_editor::surface::{Host as SurfaceHost, PaintFrame};

struct FlutterHostState {
    fonts: HashMap<u64, (Vec<u8>, FontData)>,
    rgba: Vec<u8>,
    width: u32,
    height: u32,
}

fn find_first_existing(candidates: &[&str]) -> Option<Vec<u8>> {
    candidates.iter().find_map(|p| fs::read(p).ok())
}

impl FlutterHostState {
    fn new() -> Self {
        let mut fonts = HashMap::new();
        if let Some(latin) = find_first_existing(&[
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
            "/System/Library/Fonts/Helvetica.ttc",
            "/System/Library/Fonts/SFNS.ttf",
            "/System/Library/Fonts/Core/Helvetica.ttc",
            "/System/Library/Fonts/Core/SFUI.ttf",
            "C:\\Windows\\Fonts\\segoeui.ttf",
        ]) {
            fonts.insert(
                1,
                (latin.clone(), FontData::new(Blob::new(Arc::new(latin)), 0)),
            );
        }
        if let Some(cjk) = find_first_existing(&[
            "/usr/share/fonts/opentype/ipafont-gothic/ipagp.ttf",
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
            "/System/Library/Fonts/PingFang.ttc",
            "/System/Library/Fonts/STHeiti Light.ttc",
            "/System/Library/Fonts/LanguageSupport/PingFang.ttc",
            "C:\\Windows\\Fonts\\msyh.ttc",
        ]) {
            fonts.insert(
                2,
                (cjk.clone(), FontData::new(Blob::new(Arc::new(cjk)), 0)),
            );
        }
        Self {
            fonts,
            rgba: Vec::new(),
            width: 0,
            height: 0,
        }
    }
}

impl servo::parley_editor::editing::Host for FlutterHostState {}

impl ConsoleHost for FlutterHostState {
    fn log(&mut self, msg: String) {
        println!("[Flutter Parley]: {msg}");
    }

    fn error(&mut self, msg: String) {
        eprintln!("[Flutter Parley Err]: {msg}");
    }
}

impl FontsHost for FlutterHostState {
    fn resolve(&mut self, query: FontQuery) -> Option<ResolvedFont> {
        let font_id = match query.sample_char {
            Some('中' | 'あ' | '한') if self.fonts.contains_key(&2) => 2,
            _ if self.fonts.contains_key(&1) => 1,
            _ => return None,
        };
        Some(ResolvedFont {
            font_id,
            face_index: 0,
        })
    }

    fn load_font_bytes(&mut self, font_id: u64) -> Option<Vec<u8>> {
        self.fonts.get(&font_id).map(|(bytes, _)| bytes.clone())
    }
}

impl SurfaceHost for FlutterHostState {
    fn present(&mut self, width: u32, height: u32, frame: PaintFrame) {
        let w = width.clamp(1, u16::MAX as u32) as u16;
        let h = height.clamp(1, u16::MAX as u32) as u16;
        let mut ctx = RenderContext::new(w, h);
        let mut resources = Resources::new();

        ctx.set_fill_rule(Fill::NonZero);
        ctx.set_paint(AlphaColor::<Srgb>::from_rgba8(
            frame.background.r,
            frame.background.g,
            frame.background.b,
            frame.background.a,
        ));
        ctx.fill_rect(&KurboRect::new(0.0, 0.0, w as f64, h as f64));

        for (rect, c) in &frame.rects {
            ctx.set_paint(AlphaColor::<Srgb>::from_rgba8(c.r, c.g, c.b, c.a));
            ctx.fill_rect(&KurboRect::new(
                rect.x as f64,
                rect.y as f64,
                (rect.x + rect.w) as f64,
                (rect.y + rect.h) as f64,
            ));
        }

        for run in &frame.glyph_runs {
            let Some((_, font_data)) = self.fonts.get(&run.font_id) else {
                continue;
            };
            ctx.set_paint(AlphaColor::<Srgb>::from_rgba8(
                run.color.r,
                run.color.g,
                run.color.b,
                run.color.a,
            ));
            ctx.glyph_run(&mut resources, font_data)
                .font_size(run.font_size)
                .hint(true)
                .fill_glyphs(run.glyphs.iter().map(|g| Glyph {
                    id: g.id,
                    x: g.x,
                    y: g.y,
                }));
        }

        ctx.flush();
        let mut pixmap = Pixmap::new(w, h);
        ctx.render(&mut pixmap, &mut resources);

        self.width = w as u32;
        self.height = h as u32;
        self.rgba.clear();
        self.rgba.reserve((self.width * self.height * 4) as usize);
        for px in pixmap.data() {
            self.rgba.extend_from_slice(&[px.r, px.g, px.b, px.a]);
        }
    }
}

pub struct ParleyFlutterHost {
    store: Store<FlutterHostState>,
    instance: ParleyEditor,
}

fn cstr_to_str<'a>(ptr: *const c_char) -> &'a str {
    if ptr.is_null() {
        return "";
    }
    unsafe { CStr::from_ptr(ptr) }.to_str().unwrap_or("")
}

fn instantiate_from_bytes(
    wasm_bytes: &[u8],
    initial_text: &str,
    width: f32,
    height: f32,
) -> *mut ParleyFlutterHost {
    let mut config = Config::new();
    config.wasm_component_model(true);
    #[cfg(target_os = "ios")]
    {
        let _ = config.target("pulley64");
    }
    let Ok(engine) = Engine::new(&config) else {
        return std::ptr::null_mut();
    };
    let Ok(component) = Component::from_binary(&engine, wasm_bytes) else {
        eprintln!("[Flutter Parley] failed to compile component bytes");
        return std::ptr::null_mut();
    };
    let mut linker = Linker::new(&engine);
    if ParleyEditor::add_to_linker::<_, HasSelf<_>>(&mut linker, |s: &mut FlutterHostState| s)
        .is_err()
    {
        return std::ptr::null_mut();
    }
    let mut store = Store::new(&engine, FlutterHostState::new());
    let Ok(instance) = ParleyEditor::instantiate(&mut store, &component, &linker) else {
        return std::ptr::null_mut();
    };
    if instance
        .servo_parley_editor_editor()
        .call_init(&mut store, initial_text, width, height)
        .is_err()
    {
        return std::ptr::null_mut();
    }

    Box::into_raw(Box::new(ParleyFlutterHost { store, instance }))
}

#[unsafe(no_mangle)]
pub extern "C" fn parley_host_new_from_bytes(
    wasm_ptr: *const u8,
    wasm_len: usize,
    initial_text: *const c_char,
    width: f32,
    height: f32,
) -> *mut ParleyFlutterHost {
    if wasm_ptr.is_null() || wasm_len == 0 {
        return std::ptr::null_mut();
    }
    let bytes = unsafe { std::slice::from_raw_parts(wasm_ptr, wasm_len) };
    instantiate_from_bytes(bytes, cstr_to_str(initial_text), width, height)
}

#[unsafe(no_mangle)]
pub extern "C" fn parley_host_new(
    wasm_path: *const c_char,
    initial_text: *const c_char,
    width: f32,
    height: f32,
) -> *mut ParleyFlutterHost {
    let wasm_path = cstr_to_str(wasm_path);
    let Ok(bytes) = fs::read(wasm_path) else {
        eprintln!("[Flutter Parley] failed to read wasm file: {wasm_path}");
        return std::ptr::null_mut();
    };
    instantiate_from_bytes(&bytes, cstr_to_str(initial_text), width, height)
}

#[unsafe(no_mangle)]
pub extern "C" fn parley_host_free(host: *mut ParleyFlutterHost) {
    if !host.is_null() {
        unsafe { drop(Box::from_raw(host)) };
    }
}

fn dispatch(host: *mut ParleyFlutterHost, event: EditorEvent) -> bool {
    let Some(host) = (unsafe { host.as_mut() }) else {
        return false;
    };
    host.instance
        .servo_parley_editor_editor()
        .call_handle_event(&mut host.store, &event)
        .map(|fb| fb.dirty)
        .unwrap_or(false)
}

#[unsafe(no_mangle)]
pub extern "C" fn parley_host_insert_text(
    host: *mut ParleyFlutterHost,
    text: *const c_char,
) -> bool {
    dispatch(host, EditorEvent::InsertText(cstr_to_str(text).to_string()))
}

#[unsafe(no_mangle)]
pub extern "C" fn parley_host_compose_update(
    host: *mut ParleyFlutterHost,
    text: *const c_char,
) -> bool {
    let s = cstr_to_str(text);
    let len = s.len() as u32;
    dispatch(host, EditorEvent::ComposeUpdate((s.to_string(), Some((len, len)))))
}

#[unsafe(no_mangle)]
pub extern "C" fn parley_host_compose_commit(
    host: *mut ParleyFlutterHost,
    text: *const c_char,
) -> bool {
    dispatch(host, EditorEvent::ComposeCommit(cstr_to_str(text).to_string()))
}

/// Action codes:
/// 1=DeleteBackward, 2=DeleteForward, 3=MoveLeft, 4=MoveRight, 5=MoveUp, 6=MoveDown,
/// 7=MoveLineStart, 8=MoveLineEnd, 9=SelectAll, 10=Undo, 11=Redo, 12=SetCursorVisible.
#[unsafe(no_mangle)]
pub extern "C" fn parley_host_action(
    host: *mut ParleyFlutterHost,
    action: u32,
    flag: bool,
) -> bool {
    let event = match action {
        1 => EditorEvent::DeleteBackward,
        2 => EditorEvent::DeleteForward,
        3 => EditorEvent::MoveLeft(flag),
        4 => EditorEvent::MoveRight(flag),
        5 => EditorEvent::MoveUp(flag),
        6 => EditorEvent::MoveDown(flag),
        7 => EditorEvent::MoveLineStart(flag),
        8 => EditorEvent::MoveLineEnd(flag),
        9 => EditorEvent::SelectAll,
        10 => EditorEvent::Undo,
        11 => EditorEvent::Redo,
        12 => EditorEvent::SetCursorVisible(flag),
        _ => return false,
    };
    dispatch(host, event)
}

/// Pointer kind: 0=Down, 1=Move, 2=Up.
#[unsafe(no_mangle)]
pub extern "C" fn parley_host_pointer(
    host: *mut ParleyFlutterHost,
    kind: u32,
    x: f32,
    y: f32,
    clicks: u32,
    shift: bool,
) -> bool {
    let event = match kind {
        0 => EditorEvent::PointerDown((x, y, clicks, shift)),
        1 => EditorEvent::PointerMove((x, y)),
        2 => EditorEvent::PointerUp,
        _ => return false,
    };
    dispatch(host, event)
}

#[unsafe(no_mangle)]
pub extern "C" fn parley_host_scroll(host: *mut ParleyFlutterHost, delta_y: f32) -> bool {
    dispatch(host, EditorEvent::Scroll(delta_y))
}

#[unsafe(no_mangle)]
pub extern "C" fn parley_host_resize(
    host: *mut ParleyFlutterHost,
    width: f32,
    height: f32,
) -> bool {
    dispatch(host, EditorEvent::Resize((width, height)))
}

#[unsafe(no_mangle)]
pub extern "C" fn parley_host_frame_ptr(host: *mut ParleyFlutterHost) -> *const u8 {
    let Some(host) = (unsafe { host.as_ref() }) else {
        return std::ptr::null();
    };
    host.store.data().rgba.as_ptr()
}

#[unsafe(no_mangle)]
pub extern "C" fn parley_host_width(host: *mut ParleyFlutterHost) -> u32 {
    unsafe { host.as_ref() }.map_or(0, |h| h.store.data().width)
}

#[unsafe(no_mangle)]
pub extern "C" fn parley_host_height(host: *mut ParleyFlutterHost) -> u32 {
    unsafe { host.as_ref() }.map_or(0, |h| h.store.data().height)
}
