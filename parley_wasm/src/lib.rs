//! WebAssembly Component Model text editor built on Parley for Servo.
//!
//! Compiled for `wasm32-unknown-unknown` with `fontique/system` disabled.
//! Font discovery and rasterization live in the Servo host (`servo:fonts` and
//! `servo:parley-editor/surface`); shaping, line breaking, bidi, selection,
//! hit-testing, undo/redo, viewport scrolling, and IME composition live here.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use fontique::{FallbackKey, GenericFamily, Script};
use linebender_resource_handle::Blob;
use parley::editing::{Cursor, Generation, Selection};
use parley::layout::{Affinity, PositionedLayoutItem};
use parley::{FontContext, LayoutContext, OverflowWrap, PlainEditor, StyleProperty};

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
const MAX_UNDO_HISTORY: usize = 256;
const SCROLLBAR_WIDTH: f32 = 6.0;
const SCROLLBAR_MARGIN: f32 = 6.0;

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
const SELECTION_UNFOCUSED_COLOR: Rgba = Rgba {
    r: 71,
    g: 85,
    b: 105,
    a: 140,
};
const CARET_COLOR: Rgba = Rgba {
    r: 248,
    g: 250,
    b: 252,
    a: 255,
};
const SCROLLBAR_COLOR: Rgba = Rgba {
    r: 100,
    g: 116,
    b: 139,
    a: 160,
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

#[derive(Clone)]
struct UndoSnapshot {
    text: String,
    anchor: usize,
    focus: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum EditGroup {
    None,
    Typing,
    Deleting,
}

struct ParleyEditorState {
    font_cx: FontContext,
    layout_cx: LayoutContext<ColorBrush>,
    editor: PlainEditor<ColorBrush>,
    /// Maps Parley's `Blob::id()` back to Servo's stable `font_id: u64`.
    blob_to_host_font: HashMap<u64, u64>,
    width: f32,
    height: f32,
    scroll_y: f32,
    pointer_down: bool,
    focused: bool,
    cursor_blink_on: bool,
    last_rendered_gen: Generation,
    undo_stack: Vec<UndoSnapshot>,
    redo_stack: Vec<UndoSnapshot>,
    last_edit_group: EditGroup,
}

impl ParleyEditorState {
    fn new(initial_text: &str, width: f32, height: f32) -> Self {
        let mut font_cx = FontContext::new();
        let mut blob_to_host_font = HashMap::new();

        // Request primary sans-serif + CJK + Emoji fonts from Servo's FontContext.
        let families_to_request: &[&[&str]] = &[
            &[
                "Inter",
                "Noto Sans",
                "DejaVu Sans",
                "Liberation Sans",
                "Ubuntu",
                "sans-serif",
            ],
            &[
                "Noto Sans CJK SC",
                "Noto Sans CJK JP",
                "WenQuanYi Micro Hei",
                "WenQuanYi Zen Hei",
                "IPAGothic",
                "PingFang SC",
                "Microsoft YaHei",
            ],
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
            for tag in [b"Latn", b"Hani", b"Hira", b"Kana", b"Hang", b"Arab", b"Hebr"] {
                font_cx.collection.append_fallbacks(
                    FallbackKey::new(Script::from_bytes(*tag), None),
                    registered_families.iter().copied(),
                );
            }
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
        // Wrap long unbroken tokens cleanly at the editor width (like CSS overflow-wrap: break-word).
        styles.insert(StyleProperty::OverflowWrap(OverflowWrap::BreakWord));

        let mut state = Self {
            font_cx,
            layout_cx: LayoutContext::new(),
            editor,
            blob_to_host_font,
            width,
            height,
            scroll_y: 0.0,
            pointer_down: false,
            focused: true,
            cursor_blink_on: true,
            last_rendered_gen: Generation::default(),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            last_edit_group: EditGroup::None,
        };
        state.editor.refresh_layout(&mut state.font_cx, &mut state.layout_cx);
        state
    }

    fn snapshot(&self) -> UndoSnapshot {
        let sel = self.editor.raw_selection();
        UndoSnapshot {
            text: self.editor.text().to_string(),
            anchor: sel.anchor().index(),
            focus: sel.focus().index(),
        }
    }

    fn push_undo_if_needed(&mut self, group: EditGroup) {
        let should_push = group == EditGroup::None || self.last_edit_group != group;
        if should_push {
            let snap = self.snapshot();
            if self.undo_stack.len() >= MAX_UNDO_HISTORY {
                self.undo_stack.remove(0);
            }
            self.undo_stack.push(snap);
        }
        self.redo_stack.clear();
        self.last_edit_group = group;
    }

    fn restore_snapshot(&mut self, snap: UndoSnapshot) {
        self.editor.set_text(&snap.text);
        self.editor.refresh_layout(&mut self.font_cx, &mut self.layout_cx);
        let len = snap.text.len();
        let anchor_idx = clamp_char_boundary(&snap.text, snap.anchor.min(len));
        let focus_idx = clamp_char_boundary(&snap.text, snap.focus.min(len));
        if let Some(layout) = self.editor.try_layout() {
            let anchor = Cursor::from_byte_index(layout, anchor_idx, Affinity::Downstream);
            let focus = Cursor::from_byte_index(layout, focus_idx, Affinity::Downstream);
            let sel = Selection::new(anchor, focus);
            self.editor
                .driver(&mut self.font_cx, &mut self.layout_cx)
                .set_selection(sel);
        }
        self.last_edit_group = EditGroup::None;
    }

    fn clamp_scroll(&mut self) {
        let content_h = self
            .editor
            .try_layout()
            .map_or(0.0, |l| l.height())
            + 2.0 * INSET;
        let max_scroll = (content_h - self.height).max(0.0);
        self.scroll_y = self.scroll_y.clamp(0.0, max_scroll);
    }

    fn ensure_caret_visible(&mut self) {
        let Some(cursor) = self.editor.cursor_geometry(2.0) else {
            return;
        };
        let top = cursor.y0 as f32 + INSET;
        let bottom = cursor.y1 as f32 + INSET;
        let pad = 16.0;
        if top - pad < self.scroll_y {
            self.scroll_y = (top - pad).max(0.0);
        } else if bottom + pad > self.scroll_y + self.height {
            self.scroll_y = bottom + pad - self.height;
        }
        self.clamp_scroll();
    }

    fn page_step_lines(&self) -> usize {
        let line_h = FONT_SIZE * 1.3;
        ((self.height - 2.0 * INSET) / line_h).floor().max(1.0) as usize
    }

    fn handle_event(&mut self, event: EditorEvent) -> EditorFeedback {
        let mut clipboard_write = None;
        let mut force_dirty = false;
        let mut keep_caret_visible = false;
        let prev_gen = self.editor.generation();

        match event {
            EditorEvent::InsertText(text) => {
                let is_single_word_char =
                    text.chars().count() == 1 && !text.chars().all(char::is_whitespace);
                self.push_undo_if_needed(if is_single_word_char {
                    EditGroup::Typing
                } else {
                    EditGroup::None
                });
                self.editor
                    .driver(&mut self.font_cx, &mut self.layout_cx)
                    .insert_or_replace_selection(&text);
                self.cursor_blink_on = true;
                keep_caret_visible = true;
            }
            EditorEvent::ComposeUpdate((text, cursor)) => {
                let mut drv = self.editor.driver(&mut self.font_cx, &mut self.layout_cx);
                if text.is_empty() {
                    drv.clear_compose();
                } else {
                    let cursor = cursor.map(|(s, e)| (s as usize, e as usize));
                    drv.set_compose(&text, cursor);
                }
                self.cursor_blink_on = true;
                keep_caret_visible = true;
                force_dirty = true;
            }
            EditorEvent::ComposeCommit(text) => {
                self.push_undo_if_needed(EditGroup::None);
                let mut drv = self.editor.driver(&mut self.font_cx, &mut self.layout_cx);
                drv.clear_compose();
                if !text.is_empty() {
                    drv.insert_or_replace_selection(&text);
                }
                self.cursor_blink_on = true;
                keep_caret_visible = true;
                force_dirty = true;
            }
            EditorEvent::ComposeCancel => {
                self.editor
                    .driver(&mut self.font_cx, &mut self.layout_cx)
                    .clear_compose();
                self.cursor_blink_on = true;
                force_dirty = true;
            }
            EditorEvent::DeleteBackward => {
                self.push_undo_if_needed(EditGroup::Deleting);
                self.editor
                    .driver(&mut self.font_cx, &mut self.layout_cx)
                    .backdelete();
                self.cursor_blink_on = true;
                keep_caret_visible = true;
            }
            EditorEvent::DeleteForward => {
                self.push_undo_if_needed(EditGroup::Deleting);
                self.editor
                    .driver(&mut self.font_cx, &mut self.layout_cx)
                    .delete();
                self.cursor_blink_on = true;
                keep_caret_visible = true;
            }
            EditorEvent::DeleteWordBackward => {
                self.push_undo_if_needed(EditGroup::None);
                self.editor
                    .driver(&mut self.font_cx, &mut self.layout_cx)
                    .backdelete_word();
                self.cursor_blink_on = true;
                keep_caret_visible = true;
            }
            EditorEvent::DeleteWordForward => {
                self.push_undo_if_needed(EditGroup::None);
                self.editor
                    .driver(&mut self.font_cx, &mut self.layout_cx)
                    .delete_word();
                self.cursor_blink_on = true;
                keep_caret_visible = true;
            }
            EditorEvent::MoveLeft(extend) => {
                self.last_edit_group = EditGroup::None;
                let mut drv = self.editor.driver(&mut self.font_cx, &mut self.layout_cx);
                if extend { drv.select_left(); } else { drv.move_left(); }
                self.cursor_blink_on = true;
                keep_caret_visible = true;
            }
            EditorEvent::MoveRight(extend) => {
                self.last_edit_group = EditGroup::None;
                let mut drv = self.editor.driver(&mut self.font_cx, &mut self.layout_cx);
                if extend { drv.select_right(); } else { drv.move_right(); }
                self.cursor_blink_on = true;
                keep_caret_visible = true;
            }
            EditorEvent::MoveUp(extend) => {
                self.last_edit_group = EditGroup::None;
                let mut drv = self.editor.driver(&mut self.font_cx, &mut self.layout_cx);
                if extend { drv.select_up(); } else { drv.move_up(); }
                self.cursor_blink_on = true;
                keep_caret_visible = true;
            }
            EditorEvent::MoveDown(extend) => {
                self.last_edit_group = EditGroup::None;
                let mut drv = self.editor.driver(&mut self.font_cx, &mut self.layout_cx);
                if extend { drv.select_down(); } else { drv.move_down(); }
                self.cursor_blink_on = true;
                keep_caret_visible = true;
            }
            EditorEvent::MoveWordLeft(extend) => {
                self.last_edit_group = EditGroup::None;
                let mut drv = self.editor.driver(&mut self.font_cx, &mut self.layout_cx);
                if extend { drv.select_word_left(); } else { drv.move_word_left(); }
                self.cursor_blink_on = true;
                keep_caret_visible = true;
            }
            EditorEvent::MoveWordRight(extend) => {
                self.last_edit_group = EditGroup::None;
                let mut drv = self.editor.driver(&mut self.font_cx, &mut self.layout_cx);
                if extend { drv.select_word_right(); } else { drv.move_word_right(); }
                self.cursor_blink_on = true;
                keep_caret_visible = true;
            }
            EditorEvent::MoveLineStart(extend) => {
                self.last_edit_group = EditGroup::None;
                let mut drv = self.editor.driver(&mut self.font_cx, &mut self.layout_cx);
                if extend { drv.select_to_line_start(); } else { drv.move_to_line_start(); }
                self.cursor_blink_on = true;
                keep_caret_visible = true;
            }
            EditorEvent::MoveLineEnd(extend) => {
                self.last_edit_group = EditGroup::None;
                let mut drv = self.editor.driver(&mut self.font_cx, &mut self.layout_cx);
                if extend { drv.select_to_line_end(); } else { drv.move_to_line_end(); }
                self.cursor_blink_on = true;
                keep_caret_visible = true;
            }
            EditorEvent::MoveTextStart(extend) => {
                self.last_edit_group = EditGroup::None;
                let mut drv = self.editor.driver(&mut self.font_cx, &mut self.layout_cx);
                if extend { drv.select_to_text_start(); } else { drv.move_to_text_start(); }
                self.cursor_blink_on = true;
                keep_caret_visible = true;
            }
            EditorEvent::MoveTextEnd(extend) => {
                self.last_edit_group = EditGroup::None;
                let mut drv = self.editor.driver(&mut self.font_cx, &mut self.layout_cx);
                if extend { drv.select_to_text_end(); } else { drv.move_to_text_end(); }
                self.cursor_blink_on = true;
                keep_caret_visible = true;
            }
            EditorEvent::PageUp(extend) => {
                self.last_edit_group = EditGroup::None;
                let steps = self.page_step_lines();
                let mut drv = self.editor.driver(&mut self.font_cx, &mut self.layout_cx);
                for _ in 0..steps {
                    if extend { drv.select_up(); } else { drv.move_up(); }
                }
                self.cursor_blink_on = true;
                keep_caret_visible = true;
            }
            EditorEvent::PageDown(extend) => {
                self.last_edit_group = EditGroup::None;
                let steps = self.page_step_lines();
                let mut drv = self.editor.driver(&mut self.font_cx, &mut self.layout_cx);
                for _ in 0..steps {
                    if extend { drv.select_down(); } else { drv.move_down(); }
                }
                self.cursor_blink_on = true;
                keep_caret_visible = true;
            }
            EditorEvent::SelectAll => {
                self.last_edit_group = EditGroup::None;
                self.editor
                    .driver(&mut self.font_cx, &mut self.layout_cx)
                    .select_all();
                self.cursor_blink_on = true;
            }
            EditorEvent::CollapseSelection => {
                self.last_edit_group = EditGroup::None;
                if self.editor.is_composing() {
                    self.editor
                        .driver(&mut self.font_cx, &mut self.layout_cx)
                        .clear_compose();
                } else {
                    self.editor
                        .driver(&mut self.font_cx, &mut self.layout_cx)
                        .collapse_selection();
                }
                self.cursor_blink_on = true;
                force_dirty = true;
            }
            EditorEvent::Undo => {
                if let Some(prev) = self.undo_stack.pop() {
                    let current = self.snapshot();
                    self.redo_stack.push(current);
                    self.restore_snapshot(prev);
                    self.cursor_blink_on = true;
                    keep_caret_visible = true;
                    force_dirty = true;
                }
            }
            EditorEvent::Redo => {
                if let Some(next) = self.redo_stack.pop() {
                    let current = self.snapshot();
                    self.undo_stack.push(current);
                    self.restore_snapshot(next);
                    self.cursor_blink_on = true;
                    keep_caret_visible = true;
                    force_dirty = true;
                }
            }
            EditorEvent::PointerDown((x, y, click_count, shift)) => {
                self.last_edit_group = EditGroup::None;
                self.pointer_down = true;
                self.cursor_blink_on = true;
                let lx = x - INSET;
                let ly = y + self.scroll_y - INSET;
                let mut drv = self.editor.driver(&mut self.font_cx, &mut self.layout_cx);
                match click_count {
                    2 => drv.select_word_at_point(lx, ly),
                    3 => drv.select_line_at_point(lx, ly),
                    _ if shift => drv.shift_click_extension(lx, ly),
                    _ => drv.move_to_point(lx, ly),
                }
                force_dirty = true;
            }
            EditorEvent::PointerMove((x, y)) => {
                if self.pointer_down {
                    let lx = x - INSET;
                    let ly = y + self.scroll_y - INSET;
                    self.editor
                        .driver(&mut self.font_cx, &mut self.layout_cx)
                        .extend_selection_to_point(lx, ly);
                    self.cursor_blink_on = true;
                    keep_caret_visible = true;
                }
            }
            EditorEvent::PointerUp => {
                self.pointer_down = false;
            }
            EditorEvent::Scroll(delta_y) => {
                let old_scroll = self.scroll_y;
                self.scroll_y += delta_y;
                self.clamp_scroll();
                if (self.scroll_y - old_scroll).abs() > 0.01 {
                    force_dirty = true;
                }
            }
            EditorEvent::Resize((w, h)) => {
                self.width = w.max(1.0);
                self.height = h.max(1.0);
                self.editor.set_width(Some((self.width - 2.0 * INSET).max(1.0)));
                self.editor.refresh_layout(&mut self.font_cx, &mut self.layout_cx);
                self.clamp_scroll();
                force_dirty = true;
            }
            EditorEvent::SetCursorVisible(visible) => {
                if self.cursor_blink_on != visible {
                    self.cursor_blink_on = visible;
                    force_dirty = true;
                }
            }
            EditorEvent::SetFocused(focused) => {
                if self.focused != focused {
                    self.focused = focused;
                    self.cursor_blink_on = focused;
                    force_dirty = true;
                }
            }
            EditorEvent::Copy => {
                clipboard_write = self.editor.selected_text().map(str::to_string);
            }
            EditorEvent::Cut => {
                if let Some(selected) = self.editor.selected_text().map(str::to_string) {
                    self.push_undo_if_needed(EditGroup::None);
                    clipboard_write = Some(selected);
                    self.editor
                        .driver(&mut self.font_cx, &mut self.layout_cx)
                        .delete_selection();
                    self.cursor_blink_on = true;
                    keep_caret_visible = true;
                }
            }
            EditorEvent::Paste(text) => {
                if !text.is_empty() {
                    self.push_undo_if_needed(EditGroup::None);
                    self.editor
                        .driver(&mut self.font_cx, &mut self.layout_cx)
                        .insert_or_replace_selection(&text);
                    self.cursor_blink_on = true;
                    keep_caret_visible = true;
                }
            }
        }

        if keep_caret_visible {
            self.ensure_caret_visible();
        }

        let caret_rect = self.editor.cursor_geometry(2.0).map(|b| EditRect {
            x: b.x0 as f32 + INSET,
            y: b.y0 as f32 + INSET - self.scroll_y,
            w: (b.x1 - b.x0).max(2.0) as f32,
            h: (b.y1 - b.y0) as f32,
        });

        let dirty = force_dirty || self.editor.generation() != prev_gen;
        EditorFeedback {
            caret_rect,
            clipboard_write,
            dirty,
        }
    }

    fn render(&mut self) {
        let scroll_y = self.scroll_y;
        let view_top = scroll_y - INSET - 40.0;
        let view_bottom = scroll_y + self.height + 40.0;

        let mut rects = Vec::new();
        let sel_color = if self.focused {
            SELECTION_COLOR
        } else {
            SELECTION_UNFOCUSED_COLOR
        };
        self.editor.selection_geometry_with(|bbox, _| {
            let y0 = bbox.y0 as f32 + INSET - scroll_y;
            let h = (bbox.y1 - bbox.y0) as f32;
            if y0 + h >= 0.0 && y0 <= self.height {
                rects.push((
                    Rect {
                        x: bbox.x0 as f32 + INSET,
                        y: y0,
                        w: (bbox.x1 - bbox.x0) as f32,
                        h,
                    },
                    sel_color,
                ));
            }
        });

        if self.focused
            && self.cursor_blink_on
            && self.editor.is_cursor_visible()
            && let Some(cursor) = self.editor.cursor_geometry(2.0)
        {
            let y0 = cursor.y0 as f32 + INSET - scroll_y;
            let h = (cursor.y1 - cursor.y0) as f32;
            if y0 + h >= 0.0 && y0 <= self.height {
                rects.push((
                    Rect {
                        x: cursor.x0 as f32 + INSET,
                        y: y0,
                        w: (cursor.x1 - cursor.x0).max(2.0) as f32,
                        h,
                    },
                    CARET_COLOR,
                ));
            }
        }

        let mut glyph_runs = Vec::new();
        let layout = self.editor.layout(&mut self.font_cx, &mut self.layout_cx);
        let content_height = layout.height() + 2.0 * INSET;

        for line in layout.lines() {
            let metrics = line.metrics();
            let line_top = metrics.baseline - metrics.line_height;
            let line_bottom = metrics.baseline + metrics.line_height;
            // Viewport culling: O(visible lines) rendering for large documents.
            if line_bottom < view_top {
                continue;
            }
            if line_top > view_bottom {
                break;
            }

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
                    let fm = run.font_metrics();
                    let offset = underline.offset.unwrap_or(fm.underline_offset);
                    let thickness = underline.size.unwrap_or(fm.underline_size).max(1.5);
                    let y = glyph_run.baseline() - offset + INSET - scroll_y;
                    rects.push((
                        Rect {
                            x: glyph_run.offset() + INSET,
                            y,
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
                        y: g.y + INSET - scroll_y,
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

        // Subtle scrollbar thumb when content exceeds viewport height.
        if content_height > self.height {
            let track_h = (self.height - 2.0 * SCROLLBAR_MARGIN).max(1.0);
            let thumb_h = (track_h * (self.height / content_height)).clamp(24.0, track_h);
            let max_scroll = (content_height - self.height).max(1.0);
            let thumb_y =
                SCROLLBAR_MARGIN + (scroll_y / max_scroll) * (track_h - thumb_h);
            rects.push((
                Rect {
                    x: self.width - SCROLLBAR_WIDTH - SCROLLBAR_MARGIN,
                    y: thumb_y,
                    w: SCROLLBAR_WIDTH,
                    h: thumb_h,
                },
                SCROLLBAR_COLOR,
            ));
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

fn clamp_char_boundary(s: &str, mut idx: usize) -> usize {
    idx = idx.min(s.len());
    while idx > 0 && !s.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
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
