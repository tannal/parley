//! WebAssembly Component Model text editor built on Parley for Servo.
//!
//! Compiled for `wasm32-unknown-unknown` with `fontique/system` disabled.
//! Font discovery and rasterization live in the Servo host (`servo:fonts` and
//! `servo:parley-editor/surface`); shaping, line breaking, bidi, selection,
//! hit-testing, and IME composition live here inside Parley.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use fontique::{FallbackKey, GenericFamily};
use linebender_resource_handle::Blob;
use parley::editing::Generation;
use parley::layout::PositionedLayoutItem;
use parley::{FontContext, LayoutContext, PlainEditor, StyleProperty};

wit_bindgen::generate!({
    world: "parley-editor",
    path: "wit/parley_editor.wit",
});

use exports::servo::parley_editor::editor::Guest as EditorGuest;
use servo::parley_editor::console;
use servo::parley_editor::editing::{EditorEvent, EditorFeedback, Rect as EditRect};
use servo::parley_editor::fonts::{self, FontQuery};
use servo::parley_editor::surface::{self, Glyph, GlyphRun, PaintFrame, Rect, Rgba};

const INSET: f32 = 24.0;
const FONT_SIZE: f32 = 20.0;
const BG_COLOR: Rgba = Rgba {
    r: 15,
    g: 23,
    b: 42,
    a: 255,
};
const TEXT_COLOR: Rgba = Rgba {
    r: 226,
    g: 232,
    b: 240,
    a: 255,
};
const SELECTION_COLOR: Rgba = Rgba {
    r: 79,
    g: 70,
    b: 229,
    a: 180,
};
const CARET_COLOR: Rgba = Rgba {
    r: 248,
    g: 250,
    b: 252,
    a: 255,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ColorBrush([u8; 4]);

impl ColorBrush {
    const fn from_rgba(c: Rgba) -> Self {
        Self([c.r, c.g, c.b, c.a])
    }

    const fn to_rgba(self) -> Rgba {
        Rgba {
            r: self.0[0],
            g: self.0[1],
            b: self.0[2],
            a: self.0[3],
        }
    }
}

impl Default for ColorBrush {
    fn default() -> Self {
        Self::from_rgba(TEXT_COLOR)
    }
}

struct ParleyEditorState {
    font_cx: FontContext,
    layout_cx: LayoutContext<ColorBrush>,
    editor: PlainEditor<ColorBrush>,
    /// Maps Parley's `Blob::id()` back to Servo's stable `font_id: u64`.
    blob_to_host_font: HashMap<u64, u64>,
    width: f32,
    height: f32,
    pointer_down: bool,
    last_rendered_gen: Generation,
}

impl ParleyEditorState {
    fn new(initial_text: &str, width: f32, height: f32) -> Self {
        let mut font_cx = FontContext::new();
        let mut blob_to_host_font = HashMap::new();

        // Request primary sans-serif + CJK/fallback fonts from Servo's FontContext.
        let families_to_request: &[&[&str]] = &[
            &["Inter", "Noto Sans", "DejaVu Sans", "Liberation Sans", "sans-serif"],
            &["Noto Sans CJK SC", "Noto Sans CJK JP", "PingFang SC", "Microsoft YaHei"],
            &["Noto Color Emoji", "Apple Color Emoji", "Segoe UI Emoji"],
        ];

        let mut registered_families = Vec::new();
        for stack in families_to_request {
            let query = FontQuery {
                families: stack.iter().map(|s| (*s).to_string()).collect(),
                weight: 400.0,
                stretch: 1.0,
                italic: false,
            };
            let Some(resolved) = fonts::resolve(&query) else {
                continue;
            };
            if blob_to_host_font.values().any(|&id| id == resolved.font_id) {
                continue;
            }
            let Some(bytes) = fonts::load_font_bytes(resolved.font_id) else {
                continue;
            };
            let blob = Blob::new(Arc::new(bytes));
            let blob_id = blob.id();
            blob_to_host_font.insert(blob_id, resolved.font_id);
            let added = font_cx.collection.register_fonts(blob, None);
            for (family_id, _) in added {
                if !registered_families.contains(&family_id) {
                    registered_families.push(family_id);
                }
            }
        }

        if !registered_families.is_empty() {
            font_cx
                .collection
                .set_generic_families(GenericFamily::SansSerif, registered_families.iter().copied());
            font_cx
                .collection
                .set_generic_families(GenericFamily::SystemUi, registered_families.iter().copied());
            font_cx
                .collection
                .append_fallbacks(
                    FallbackKey::new(fontique::Script::from_bytes(*b"Latn"), None),
                    registered_families.iter().copied(),
                );
        } else {
            console::error("[parley.wasm] host returned no fonts; text will not render");
        }

        let mut editor = PlainEditor::new(FONT_SIZE);
        editor.set_text(initial_text);
        editor.set_scale(1.0);
        editor.set_width(Some((width - 2.0 * INSET).max(1.0)));
        let styles = editor.edit_styles();
        styles.insert(GenericFamily::SansSerif.into());
        styles.insert(StyleProperty::Brush(ColorBrush::from_rgba(TEXT_COLOR)));

        let mut state = Self {
            font_cx,
            layout_cx: LayoutContext::new(),
            editor,
            blob_to_host_font,
            width,
            height,
            pointer_down: false,
            last_rendered_gen: Generation::default(),
        };
        state.editor.refresh_layout(&mut state.font_cx, &mut state.layout_cx);
        state
    }

    fn handle_event(&mut self, event: EditorEvent) -> EditorFeedback {
        let mut clipboard_write = None;
        let prev_gen = self.editor.generation();

        match event {
            EditorEvent::InsertText(text) => {
                self.editor
                    .driver(&mut self.font_cx, &mut self.layout_cx)
                    .insert_or_replace_selection(&text);
            }
            EditorEvent::ComposeUpdate((text, cursor)) => {
                let mut drv = self.editor.driver(&mut self.font_cx, &mut self.layout_cx);
                if text.is_empty() {
                    drv.clear_compose();
                } else {
                    let cursor = cursor.map(|(s, e)| (s as usize, e as usize));
                    drv.set_compose(&text, cursor);
                }
            }
            EditorEvent::ComposeCommit(text) => {
                let mut drv = self.editor.driver(&mut self.font_cx, &mut self.layout_cx);
                drv.clear_compose();
                drv.insert_or_replace_selection(&text);
            }
            EditorEvent::ComposeCancel => {
                self.editor
                    .driver(&mut self.font_cx, &mut self.layout_cx)
                    .clear_compose();
            }
            EditorEvent::DeleteBackward => {
                self.editor
                    .driver(&mut self.font_cx, &mut self.layout_cx)
                    .backdelete();
            }
            EditorEvent::DeleteForward => {
                self.editor
                    .driver(&mut self.font_cx, &mut self.layout_cx)
                    .delete();
            }
            EditorEvent::DeleteWordBackward => {
                self.editor
                    .driver(&mut self.font_cx, &mut self.layout_cx)
                    .backdelete_word();
            }
            EditorEvent::MoveLeft(extend) => {
                let mut drv = self.editor.driver(&mut self.font_cx, &mut self.layout_cx);
                if extend { drv.select_left(); } else { drv.move_left(); }
            }
            EditorEvent::MoveRight(extend) => {
                let mut drv = self.editor.driver(&mut self.font_cx, &mut self.layout_cx);
                if extend { drv.select_right(); } else { drv.move_right(); }
            }
            EditorEvent::MoveUp(extend) => {
                let mut drv = self.editor.driver(&mut self.font_cx, &mut self.layout_cx);
                if extend { drv.select_up(); } else { drv.move_up(); }
            }
            EditorEvent::MoveDown(extend) => {
                let mut drv = self.editor.driver(&mut self.font_cx, &mut self.layout_cx);
                if extend { drv.select_down(); } else { drv.move_down(); }
            }
            EditorEvent::MoveWordLeft(extend) => {
                let mut drv = self.editor.driver(&mut self.font_cx, &mut self.layout_cx);
                if extend { drv.select_word_left(); } else { drv.move_word_left(); }
            }
            EditorEvent::MoveWordRight(extend) => {
                let mut drv = self.editor.driver(&mut self.font_cx, &mut self.layout_cx);
                if extend { drv.select_word_right(); } else { drv.move_word_right(); }
            }
            EditorEvent::MoveLineStart(extend) => {
                let mut drv = self.editor.driver(&mut self.font_cx, &mut self.layout_cx);
                if extend { drv.select_to_line_start(); } else { drv.move_to_line_start(); }
            }
            EditorEvent::MoveLineEnd(extend) => {
                let mut drv = self.editor.driver(&mut self.font_cx, &mut self.layout_cx);
                if extend { drv.select_to_line_end(); } else { drv.move_to_line_end(); }
            }
            EditorEvent::SelectAll => {
                self.editor
                    .driver(&mut self.font_cx, &mut self.layout_cx)
                    .select_all();
            }
            EditorEvent::PointerDown((x, y, click_count, shift)) => {
                self.pointer_down = true;
                let lx = x - INSET;
                let ly = y - INSET;
                let mut drv = self.editor.driver(&mut self.font_cx, &mut self.layout_cx);
                match click_count {
                    2 => drv.select_word_at_point(lx, ly),
                    3 => drv.select_line_at_point(lx, ly),
                    _ if shift => drv.shift_click_extension(lx, ly),
                    _ => drv.move_to_point(lx, ly),
                }
            }
            EditorEvent::PointerMove((x, y)) => {
                if self.pointer_down {
                    self.editor
                        .driver(&mut self.font_cx, &mut self.layout_cx)
                        .extend_selection_to_point(x - INSET, y - INSET);
                }
            }
            EditorEvent::PointerUp => {
                self.pointer_down = false;
            }
            EditorEvent::Resize((w, h)) => {
                self.width = w.max(1.0);
                self.height = h.max(1.0);
                self.editor.set_width(Some((self.width - 2.0 * INSET).max(1.0)));
                self.editor.refresh_layout(&mut self.font_cx, &mut self.layout_cx);
            }
            EditorEvent::Copy => {
                clipboard_write = self.editor.selected_text().map(str::to_string);
            }
            EditorEvent::Cut => {
                if let Some(selected) = self.editor.selected_text().map(str::to_string) {
                    clipboard_write = Some(selected);
                    self.editor
                        .driver(&mut self.font_cx, &mut self.layout_cx)
                        .delete_selection();
                }
            }
            EditorEvent::Paste(text) => {
                self.editor
                    .driver(&mut self.font_cx, &mut self.layout_cx)
                    .insert_or_replace_selection(&text);
            }
        }

        let caret_rect = self.editor.cursor_geometry(2.0).map(|b| EditRect {
            x: b.x0 as f32 + INSET,
            y: b.y0 as f32 + INSET,
            w: (b.x1 - b.x0) as f32,
            h: (b.y1 - b.y0) as f32,
        });

        let dirty = self.editor.generation() != prev_gen;
        EditorFeedback {
            caret_rect,
            clipboard_write,
            dirty,
        }
    }

    fn render(&mut self) {
        let mut rects = Vec::new();
        self.editor.selection_geometry_with(|bbox, _| {
            rects.push((
                Rect {
                    x: bbox.x0 as f32 + INSET,
                    y: bbox.y0 as f32 + INSET,
                    w: (bbox.x1 - bbox.x0) as f32,
                    h: (bbox.y1 - bbox.y0) as f32,
                },
                SELECTION_COLOR,
            ));
        });

        if self.editor.is_cursor_visible()
            && let Some(cursor) = self.editor.cursor_geometry(2.0)
        {
            rects.push((
                Rect {
                    x: cursor.x0 as f32 + INSET,
                    y: cursor.y0 as f32 + INSET,
                    w: (cursor.x1 - cursor.x0).max(2.0) as f32,
                    h: (cursor.y1 - cursor.y0) as f32,
                },
                CARET_COLOR,
            ));
        }

        let mut glyph_runs = Vec::new();
        let layout = self.editor.layout(&mut self.font_cx, &mut self.layout_cx);
        for line in layout.lines() {
            for item in line.items() {
                let PositionedLayoutItem::GlyphRun(glyph_run) = item else {
                    continue;
                };
                let style = glyph_run.style();
                let run = glyph_run.run();
                let blob_id = run.font().data.id();
                let Some(&host_font_id) = self.blob_to_host_font.get(&blob_id) else {
                    continue;
                };

                if let Some(underline) = &style.underline {
                    let metrics = run.font_metrics();
                    let offset = underline.offset.unwrap_or(metrics.underline_offset);
                    let thickness = underline.size.unwrap_or(metrics.underline_size).max(1.0);
                    let y = glyph_run.baseline() - offset;
                    rects.push((
                        Rect {
                            x: glyph_run.offset() + INSET,
                            y: y + INSET,
                            w: glyph_run.advance(),
                            h: thickness,
                        },
                        style.brush.to_rgba(),
                    ));
                }

                let glyphs = glyph_run
                    .positioned_glyphs()
                    .map(|g| Glyph {
                        id: g.id,
                        x: g.x + INSET,
                        y: g.y + INSET,
                    })
                    .collect();

                glyph_runs.push(GlyphRun {
                    font_id: host_font_id,
                    font_size: run.font_size(),
                    color: style.brush.to_rgba(),
                    glyphs,
                });
            }
        }

        self.last_rendered_gen = self.editor.generation();
        surface::present(
            self.width.ceil() as u32,
            self.height.ceil() as u32,
            &PaintFrame {
                background: BG_COLOR,
                rects,
                glyph_runs,
            },
        );
    }

    fn text(&self) -> String {
        self.editor.text().to_string()
    }
}

thread_local! {
    static STATE: RefCell<Option<ParleyEditorState>> = const { RefCell::new(None) };
}

struct Component;

impl EditorGuest for Component {
    fn init(initial_text: String, width: f32, height: f32) {
        console::log("[parley.wasm] init()");
        let mut state = ParleyEditorState::new(&initial_text, width, height);
        state.render();
        STATE.with(|slot| *slot.borrow_mut() = Some(state));
    }

    fn handle_event(event: EditorEvent) -> EditorFeedback {
        STATE.with(|slot| {
            let mut borrow = slot.borrow_mut();
            let Some(state) = borrow.as_mut() else {
                return EditorFeedback {
                    caret_rect: None,
                    clipboard_write: None,
                    dirty: false,
                };
            };
            let feedback = state.handle_event(event);
            if feedback.dirty {
                state.render();
            }
            feedback
        })
    }

    fn render() {
        STATE.with(|slot| {
            if let Some(state) = slot.borrow_mut().as_mut() {
                state.render();
            }
        });
    }

    fn get_text() -> String {
        STATE.with(|slot| {
            slot.borrow()
                .as_ref()
                .map(ParleyEditorState::text)
                .unwrap_or_default()
        })
    }
}

export!(Component);
