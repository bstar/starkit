// SPDX-License-Identifier: Apache-2.0
// Adapted from GPUI 0.2.2 examples/input.rs (Zed Industries).
// Changes: controlled state/events, scoped bindings, STAR themes and single-line normalization.
//! Native selection and IME input; callers retain validation and workflow ownership.
use std::ops::Range;

use gpui::{
    actions, div, fill, hsla, point, prelude::*, px, relative, size, App, Bounds, ClipboardItem,
    Context, CursorStyle, ElementId, ElementInputHandler, Entity, EntityInputHandler, FocusHandle,
    Focusable, GlobalElementId, KeyBinding, Keystroke, LayoutId, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point, ShapedLine, SharedString, Style,
    TextRun, UTF16Selection, UnderlineStyle, Window,
};
use unicode_segmentation::*;

actions!(
    star_native_input,
    [
        Backspace,
        Delete,
        Left,
        Right,
        SelectLeft,
        SelectRight,
        SelectAll,
        Home,
        End,
        ShowCharacterPalette,
        Paste,
        Cut,
        Copy,
        Submit,
        Cancel,
        Mode,
        Clear,
    ]
);

pub struct NativeInput {
    navigation: bool,
    scroll_x: Pixels,
    secret: bool,
    tokens: super::super::Tokens,
    focus_handle: FocusHandle,
    content: SharedString,
    placeholder: SharedString,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    last_layout: Option<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
    is_selecting: bool,
}

impl NativeInput {
    pub fn new(
        text: &str,
        cursor: usize,
        tokens: super::super::Tokens,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut cursor = cursor.min(text.len());
        while !text.is_char_boundary(cursor) {
            cursor -= 1;
        }
        Self {
            navigation: false,
            scroll_x: px(0.),
            secret: false,
            tokens,
            focus_handle: cx.focus_handle(),
            content: text.to_owned().into(),
            placeholder: "".into(),
            selected_range: cursor..cursor,
            selection_reversed: false,
            marked_range: None,
            last_layout: None,
            last_bounds: None,
            is_selecting: false,
        }
    }
    pub fn sync(&mut self, text: &str, cursor: usize, tokens: super::super::Tokens) {
        self.tokens = tokens;
        if self.content.as_ref() != text {
            self.content = text.to_owned().into();
            let mut cursor = cursor.min(text.len());
            while !text.is_char_boundary(cursor) {
                cursor -= 1;
            }
            self.selected_range = cursor..cursor;
            self.marked_range = None;
            self.selection_reversed = false;
        }
    }
    pub fn text(&self) -> &str {
        &self.content
    }
    pub fn cursor(&self) -> usize {
        self.cursor_offset()
    }
    pub fn set_secret(&mut self, secret: bool) {
        self.secret = secret;
    }
    pub fn set_navigation(&mut self, navigation: bool) {
        self.navigation = navigation;
    }
    fn navigation_key(
        &mut self,
        event: &gpui::KeyDownEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if matches!(
            event.keystroke.key.as_str(),
            "up" | "down"
                | "pageup"
                | "pagedown"
                | "f1"
                | "f2"
                | "f3"
                | "f4"
                | "f5"
                | "f6"
                | "f7"
                | "f8"
                | "f9"
                | "f10"
        ) {
            cx.emit(InputEvent::Unhandled(event.keystroke.clone()));
        }
    }
    fn clear(&mut self, _: &Clear, window: &mut Window, cx: &mut Context<Self>) {
        self.selected_range = 0..self.content.len();
        self.replace_text_in_range(None, "", window, cx);
    }
    fn changed(&self, cx: &mut Context<Self>) {
        cx.emit(InputEvent::Edited {
            text: self.content.to_string(),
            cursor: self.cursor_offset(),
        });
        cx.notify();
    }
    fn submit(&mut self, _: &Submit, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(InputEvent::Submit);
    }
    fn cancel(&mut self, _: &Cancel, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(InputEvent::Cancel);
    }
    fn mode(&mut self, _: &Mode, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(InputEvent::Mode);
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.previous_boundary(self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.start, cx)
        }
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.next_boundary(self.selected_range.end), cx);
        } else {
            self.move_to(self.selected_range.end, cx)
        }
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_boundary(self.cursor_offset()), cx);
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_boundary(self.cursor_offset()), cx);
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
        self.select_to(self.content.len(), cx)
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        if self.navigation {
            cx.emit(InputEvent::Unhandled(Keystroke::parse("home").unwrap()));
        } else {
            self.move_to(0, cx);
        }
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        if self.navigation {
            cx.emit(InputEvent::Unhandled(Keystroke::parse("end").unwrap()));
        } else {
            self.move_to(self.content.len(), cx);
        }
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.select_to(self.previous_boundary(self.cursor_offset()), cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.select_to(self.next_boundary(self.cursor_offset()), cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.is_selecting = true;

        if event.modifiers.shift {
            self.select_to(self.index_for_mouse_position(event.position), cx);
        } else {
            self.move_to(self.index_for_mouse_position(event.position), cx)
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _window: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_selecting {
            self.select_to(self.index_for_mouse_position(event.position), cx);
        }
    }

    fn show_character_palette(
        &mut self,
        _: &ShowCharacterPalette,
        window: &mut Window,
        _: &mut Context<Self>,
    ) {
        window.show_character_palette();
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.replace_text_in_range(None, &text.replace("\n", " "), window, cx);
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.secret && !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
        }
    }
    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if !self.secret && !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
            self.replace_text_in_range(None, "", window, cx)
        }
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.selected_range = offset..offset;
        self.changed(cx)
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        if self.content.is_empty() {
            return 0;
        }

        let (Some(bounds), Some(line)) = (self.last_bounds.as_ref(), self.last_layout.as_ref())
        else {
            return 0;
        };
        if position.y < bounds.top() {
            return 0;
        }
        if position.y > bounds.bottom() {
            return self.content.len();
        }
        byte_boundary(
            &self.content,
            line.closest_index_for_x(position.x - bounds.left()),
        )
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        if self.selection_reversed {
            self.selected_range.start = offset
        } else {
            self.selected_range.end = offset
        };
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
        self.changed(cx)
    }

    fn offset_from_utf16(&self, offset: usize) -> usize {
        utf8_offset(&self.content, offset)
    }
    fn offset_to_utf16(&self, offset: usize) -> usize {
        utf16_offset(&self.content, offset)
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range_utf16: &Range<usize>) -> Range<usize> {
        let start = self.offset_from_utf16(range_utf16.start);
        let end = self.offset_from_utf16(range_utf16.end);
        start.min(end)..start.max(end)
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .rev()
            .find_map(|(idx, _)| (idx < offset).then_some(idx))
            .unwrap_or(0)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .find_map(|(idx, _)| (idx > offset).then_some(idx))
            .unwrap_or(self.content.len())
    }
}

impl EntityInputHandler for NativeInput {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.content[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected_range),
            reversed: self.selection_reversed,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.marked_range
            .as_ref()
            .map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.marked_range = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let new_text = new_text.replace(['\n', '\r'], " ");
        let new_text = new_text.as_str();
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());

        self.content =
            (self.content[0..range.start].to_owned() + new_text + &self.content[range.end..])
                .into();
        self.selected_range = range.start + new_text.len()..range.start + new_text.len();
        self.marked_range.take();
        self.changed(cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let new_text = new_text.replace(['\n', '\r'], " ");
        let new_text = new_text.as_str();
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());

        self.content =
            (self.content[0..range.start].to_owned() + new_text + &self.content[range.end..])
                .into();
        if !new_text.is_empty() {
            self.marked_range = Some(range.start..range.start + new_text.len());
        } else {
            self.marked_range = None;
        }
        self.selected_range = new_selected_range_utf16
            .as_ref()
            .map(|range_utf16| {
                let start = utf8_offset(new_text, range_utf16.start);
                let end = utf8_offset(new_text, range_utf16.end);
                start.min(end) + range.start..start.max(end) + range.start
            })
            .unwrap_or_else(|| range.start + new_text.len()..range.start + new_text.len());

        self.changed(cx);
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let last_layout = self.last_layout.as_ref()?;
        let range = self.range_from_utf16(&range_utf16);
        Some(Bounds::from_corners(
            point(
                bounds.left() - self.scroll_x + last_layout.x_for_index(range.start),
                bounds.top(),
            ),
            point(
                bounds.left() - self.scroll_x + last_layout.x_for_index(range.end),
                bounds.bottom(),
            ),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: gpui::Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let line_point = self.last_bounds?.localize(&point)?;
        let last_layout = self.last_layout.as_ref()?;

        if !self.secret {
            debug_assert_eq!(last_layout.text, self.content);
        }
        let utf8_index = byte_boundary(&self.content, last_layout.index_for_x(line_point.x)?);
        Some(self.offset_to_utf16(utf8_index))
    }
}

struct TextElement {
    input: Entity<NativeInput>,
}

struct PrepaintState {
    line: Option<ShapedLine>,
    cursor: Option<PaintQuad>,
    selection: Option<PaintQuad>,
    scroll_x: Pixels,
}

impl IntoElement for TextElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = window.line_height().into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let input = self.input.read(cx);
        let content = if input.secret {
            SharedString::from("*".repeat(input.content.len()))
        } else {
            input.content.clone()
        };
        let selected_range = input.selected_range.clone();
        let cursor = input.cursor_offset();
        let style = window.text_style();

        let (display_text, text_color) = if content.is_empty() {
            (input.placeholder.clone(), hsla(0., 0., 0., 0.2))
        } else {
            (content, style.color)
        };

        let run = TextRun {
            len: display_text.len(),
            font: style.font(),
            color: text_color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let runs = if let Some(marked_range) = input.marked_range.as_ref() {
            vec![
                TextRun {
                    len: marked_range.start,
                    ..run.clone()
                },
                TextRun {
                    len: marked_range.end - marked_range.start,
                    underline: Some(UnderlineStyle {
                        color: Some(run.color),
                        thickness: px(1.0),
                        wavy: false,
                    }),
                    ..run.clone()
                },
                TextRun {
                    len: display_text.len() - marked_range.end,
                    ..run
                },
            ]
            .into_iter()
            .filter(|run| run.len > 0)
            .collect()
        } else {
            vec![run]
        };

        let font_size = style.font_size.to_pixels(window.rem_size());
        let line = window
            .text_system()
            .shape_line(display_text, font_size, &runs, None);

        let cursor_pos = line.x_for_index(cursor);
        let room = (bounds.size.width - px(4.)).max(px(1.));
        let scroll_x = if cursor_pos < input.scroll_x {
            cursor_pos
        } else if cursor_pos > input.scroll_x + room {
            cursor_pos - room
        } else {
            input.scroll_x
        };
        let (selection, cursor) = if selected_range.is_empty() {
            (
                None,
                Some(fill(
                    Bounds::new(
                        point(bounds.left() - scroll_x + cursor_pos, bounds.top()),
                        size(px(2.), bounds.bottom() - bounds.top()),
                    ),
                    super::rgb24(input.tokens.foreground),
                )),
            )
        } else {
            (
                Some(fill(
                    Bounds::from_corners(
                        point(
                            bounds.left() - scroll_x + line.x_for_index(selected_range.start),
                            bounds.top(),
                        ),
                        point(
                            bounds.left() - scroll_x + line.x_for_index(selected_range.end),
                            bounds.bottom(),
                        ),
                    ),
                    super::rgb24(input.tokens.selected),
                )),
                None,
            )
        };
        PrepaintState {
            line: Some(line),
            cursor,
            selection,
            scroll_x,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.input.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );
        let line = prepaint.line.take().unwrap();
        let origin = point(bounds.left() - prepaint.scroll_x, bounds.top());
        window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
            if let Some(selection) = prepaint.selection.take() {
                window.paint_quad(selection);
            }
            let _ = line.paint(origin, window.line_height(), window, cx);
            if focus_handle.is_focused(window) {
                if let Some(cursor) = prepaint.cursor.take() {
                    window.paint_quad(cursor);
                }
            }
        });
        self.input.update(cx, |input, _cx| {
            input.last_layout = Some(line);
            input.last_bounds = Some(Bounds::new(origin, bounds.size));
            input.scroll_x = prepaint.scroll_x;
        });
    }
}

impl Render for NativeInput {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .w_full()
            .min_w(px(0.))
            .overflow_hidden()
            .rounded_md()
            .border_1()
            .border_color(super::rgb24(self.tokens.accent))
            .key_context("STARkitNativeInput")
            .track_focus(&self.focus_handle(cx))
            .cursor(CursorStyle::IBeam)
            .on_key_down(cx.listener(Self::navigation_key))
            .on_action(cx.listener(Self::clear))
            .on_action(cx.listener(Self::submit))
            .on_action(cx.listener(Self::cancel))
            .on_action(cx.listener(Self::mode))
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::show_character_palette))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::copy))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .bg(super::rgb24(self.tokens.background))
            .line_height(px(24.))
            .text_size(px(14.))
            .child(
                div()
                    .h(px(24. + 4. * 2.))
                    .w_full()
                    .p(px(4.))
                    .bg(super::rgb24(self.tokens.background))
                    .child(TextElement { input: cx.entity() }),
            )
    }
}

impl Focusable for NativeInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

#[derive(Clone, Debug)]
pub enum InputEvent {
    Edited { text: String, cursor: usize },
    Submit,
    Cancel,
    Mode,
    Unhandled(Keystroke),
}
impl gpui::EventEmitter<InputEvent> for NativeInput {}

/// Install only in a desktop application; bindings apply only to this focused field.
pub fn install_bindings(cx: &mut App) {
    let context = Some("STARkitNativeInput");
    cx.bind_keys([
        KeyBinding::new("ctrl-u", Clear, context),
        KeyBinding::new("enter", Submit, context),
        KeyBinding::new("escape", Cancel, context),
        KeyBinding::new("tab", Mode, context),
        KeyBinding::new("backspace", Backspace, context),
        KeyBinding::new("delete", Delete, context),
        KeyBinding::new("left", Left, context),
        KeyBinding::new("right", Right, context),
        KeyBinding::new("shift-left", SelectLeft, context),
        KeyBinding::new("shift-right", SelectRight, context),
        KeyBinding::new("ctrl-a", SelectAll, context),
        KeyBinding::new("cmd-a", SelectAll, context),
        KeyBinding::new("ctrl-v", Paste, context),
        KeyBinding::new("cmd-v", Paste, context),
        KeyBinding::new("ctrl-c", Copy, context),
        KeyBinding::new("cmd-c", Copy, context),
        KeyBinding::new("ctrl-x", Cut, context),
        KeyBinding::new("cmd-x", Cut, context),
        KeyBinding::new("home", Home, context),
        KeyBinding::new("end", End, context),
    ]);
}

fn byte_boundary(text: &str, offset: usize) -> usize {
    let mut offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}
fn utf8_offset(text: &str, offset: usize) -> usize {
    let mut bytes = 0;
    let mut units = 0;
    for character in text.chars() {
        if units >= offset {
            break;
        }
        units += character.len_utf16();
        bytes += character.len_utf8();
    }
    bytes
}
fn utf16_offset(text: &str, offset: usize) -> usize {
    let mut bytes = 0;
    let mut units = 0;
    for character in text.chars() {
        if bytes >= offset {
            break;
        }
        bytes += character.len_utf8();
        units += character.len_utf16();
    }
    units
}
#[cfg(test)]
mod tests {
    use super::{byte_boundary, utf16_offset, utf8_offset};
    use proptest::prelude::*;
    #[test]
    fn ime_offsets_preserve_surrogate_pairs_and_combining_text() {
        let text = "a界🦀e\u{301}";
        for index in text
            .char_indices()
            .map(|(index, _)| index)
            .chain([text.len()])
        {
            assert_eq!(utf8_offset(text, utf16_offset(text, index)), index);
        }
        assert_eq!(utf8_offset(text, 3), 8);
        assert_eq!(utf8_offset(text, usize::MAX), text.len());
        assert_eq!(byte_boundary(text, 6), 4);
    }
    proptest! {
        #[test]
        fn arbitrary_input_offsets_remain_on_utf8_boundaries(text in ".{0,500}", offset in any::<usize>()) {
            let boundary = byte_boundary(&text, offset);
            prop_assert!(text.is_char_boundary(boundary));
            let byte = utf8_offset(&text, offset);
            prop_assert!(byte <= text.len() && text.is_char_boundary(byte));
            prop_assert_eq!(utf8_offset(&text, utf16_offset(&text, byte)), byte);
        }
    }
}
