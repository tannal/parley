//! Zero-Servo standalone host proving that the exact same `parley.wasm`
//! Component Model binary runs outside any browser by implementing the three
//! host WIT interfaces (`console`, `fonts`, `surface`) with Wasmtime + `vello_cpu`.

use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use peniko::color::{AlphaColor, Srgb};
use peniko::kurbo::Rect as KurboRect;
use peniko::{Blob, Fill, FontData};
use vello_cpu::{Glyph, Pixmap, RenderContext, Resources};
use wasmtime::component::{Component, HasSelf, Linker};
use wasmtime::{Config, Engine, Store};

wasmtime::component::bindgen!({
    world: "parley-editor",
    path: "../../wit/parley_editor.wit",
});

use servo::parley_editor::console::Host as ConsoleHost;
use servo::parley_editor::editing::EditorEvent;
use servo::parley_editor::fonts::{FontQuery, Host as FontsHost, ResolvedFont};
use servo::parley_editor::surface::{Host as SurfaceHost, PaintFrame};

struct StandaloneHost {
    fonts: HashMap<u64, (Vec<u8>, FontData)>,
    last_frame: Option<(u32, u32, PaintFrame)>,
}

fn find_first_existing(candidates: &[&str]) -> Option<Vec<u8>> {
    candidates.iter().find_map(|p| fs::read(p).ok())
}

impl StandaloneHost {
    fn new() -> Self {
        let mut fonts = HashMap::new();
        let latin = find_first_existing(&[
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
            "/System/Library/Fonts/Helvetica.ttc",
        ])
        .expect("could not locate a Latin system font");

        fonts.insert(
            1,
            (
                latin.clone(),
                FontData::new(Blob::new(Arc::new(latin)), 0),
            ),
        );

        if let Some(cjk) = find_first_existing(&[
            "/usr/share/fonts/opentype/ipafont-gothic/ipagp.ttf",
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
            "/System/Library/Fonts/PingFang.ttc",
        ]) {
            fonts.insert(
                2,
                (
                    cjk.clone(),
                    FontData::new(Blob::new(Arc::new(cjk)), 0),
                ),
            );
        }

        Self {
            fonts,
            last_frame: None,
        }
    }
}

impl servo::parley_editor::editing::Host for StandaloneHost {}

impl ConsoleHost for StandaloneHost {
    fn log(&mut self, msg: String) {
        println!("[standalone host] {msg}");
    }

    fn error(&mut self, msg: String) {
        eprintln!("[standalone host err] {msg}");
    }
}

impl FontsHost for StandaloneHost {
    fn resolve(&mut self, query: FontQuery) -> Option<ResolvedFont> {
        let font_id = match query.sample_char {
            Some('中' | 'あ' | '한') if self.fonts.contains_key(&2) => 2,
            _ => 1,
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

impl SurfaceHost for StandaloneHost {
    fn present(&mut self, width: u32, height: u32, frame: PaintFrame) {
        self.last_frame = Some((width, height, frame));
    }
}

fn render_png(host: &StandaloneHost, out_path: &str) {
    let (w, h, frame) = host.last_frame.as_ref().expect("no frame presented");
    let mut ctx = RenderContext::new(*w as u16, *h as u16);
    let mut resources = Resources::new();

    ctx.set_fill_rule(Fill::NonZero);
    ctx.set_paint(AlphaColor::<Srgb>::from_rgba8(
        frame.background.r,
        frame.background.g,
        frame.background.b,
        frame.background.a,
    ));
    ctx.fill_rect(&KurboRect::new(0.0, 0.0, *w as f64, *h as f64));

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
        let Some((_, font_data)) = host.fonts.get(&run.font_id) else {
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
    let mut pixmap = Pixmap::new(*w as u16, *h as u16);
    ctx.render(&mut pixmap, &mut resources);

    let png_bytes = pixmap.into_png().expect("PNG encoding failed");
    fs::write(out_path, png_bytes).expect("failed to write output PNG");
    println!("Rendered standalone non-Servo PNG: {out_path} ({w}x{h})");
}

fn main() -> wasmtime::Result<()> {
    let args: Vec<String> = env::args().collect();
    let wasm_path = args.get(1).map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/parley.wasm")
    });
    let out_path = args
        .get(2)
        .map(String::as_str)
        .unwrap_or("parley_standalone.png");

    let mut config = Config::new();
    config.wasm_component_model(true);
    let engine = Engine::new(&config)?;
    let component = Component::from_file(&engine, &wasm_path)?;

    let mut linker = Linker::new(&engine);
    ParleyEditor::add_to_linker::<_, HasSelf<_>>(&mut linker, |s: &mut StandaloneHost| s)?;

    let mut store = Store::new(&engine, StandaloneHost::new());
    let instance = ParleyEditor::instantiate(&mut store, &component, &linker)?;
    let editor = instance.servo_parley_editor_editor();

    editor.call_init(
        &mut store,
        "Standalone Non-Servo Host Proof (Zero Browser, Zero DOM!)\n\n\
         你好，世界！Component Model = Write Once, Run Anywhere:\n\
         Servo · Flutter · Android · iOS · macOS · Windows · Linux",
        760.0,
        340.0,
    )?;

    editor.call_handle_event(&mut store, &EditorEvent::MoveTextEnd(false))?;
    editor.call_handle_event(
        &mut store,
        &EditorEvent::InsertText("\n\nActive IME preedit: ".into()),
    )?;
    editor.call_handle_event(
        &mut store,
        &EditorEvent::ComposeUpdate(("nihao".into(), Some((5, 5)))),
    )?;

    render_png(store.data(), out_path);
    Ok(())
}
