//! A read-only, syntax highlighted code view widget.
//!
//! Draws a file with a line number gutter and highlighted ranges of lines ([`Mark`]s),
//! and supports mouse text selection (drag, double-click word, triple-click line),
//! Ctrl+C, horizontal scrolling (Shift+wheel / horizontal wheel / a draggable scrollbar
//! that sticks to the bottom of the visible area) and optional soft wrapping of long lines.
//!
//! Lines can carry [`Insert`]s, such as a walkthrough element's chat box: a badge at the
//! right end of the line, and a body shown in a gap opened under the line while it is
//! expanded. The view lays out, draws and routes events to these child widgets.
//!
//! Text layout assumes a monospace font: every char occupies one cell, tabs are
//! expanded beforehand (see [`crate::highlight::expand_tabs`]).

use crate::highlight::{self, LineSpans};
use iced::advanced::clipboard::{self, Clipboard};
use iced::advanced::layout::{self, Layout};
use iced::advanced::overlay;
use iced::advanced::renderer;
use iced::advanced::text::{self, Paragraph as _};
use iced::advanced::widget::{self, Tree, Widget, tree};
use iced::advanced::{Shell, mouse};
use iced::alignment;
use iced::font::{Style as FontStyle, Weight};
use iced::keyboard;
use iced::{Background, Border, Color, Element, Event, Font, Length, Pixels, Point, Rectangle, Size, Theme, Vector};
use std::time::{Duration, Instant};

type Renderer = iced::Renderer;
type Paragraph = <Renderer as text::Renderer>::Paragraph;
use iced::advanced::Renderer as _;
use iced::advanced::text::Renderer as _;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Colors {
    pub text: Color,
    pub muted: Color,
    pub gutter_bg: Color,
    pub selection: Color,
    /// Background of the walkthrough element being looked at, and of reopened ones.
    pub mark_current: Color,
    pub mark_open: Color,
    /// The bar in front of marked lines.
    pub mark_bar: Color,
    pub mark_past_bar: Color,
    pub scrollbar: Color,
}

impl Colors {
    pub fn new(dark: bool) -> Self {
        let c = Color::from_rgb8;
        let ca = Color::from_rgba8;
        if dark {
            Colors {
                text: c(0xe6, 0xed, 0xf3),
                muted: c(0x7d, 0x85, 0x90),
                gutter_bg: c(0x0d, 0x11, 0x17),
                selection: ca(0x38, 0x8b, 0xfd, 0.35),
                mark_current: ca(0xe3, 0xb3, 0x41, 0.2),
                mark_open: ca(0xe3, 0xb3, 0x41, 0.1),
                mark_bar: c(0xe3, 0xb3, 0x41),
                mark_past_bar: ca(0xa3, 0x71, 0xf7, 0.6),
                scrollbar: ca(0xff, 0xff, 0xff, 0.25),
            }
        } else {
            Colors {
                text: c(0x1f, 0x23, 0x28),
                muted: c(0x59, 0x63, 0x6e),
                gutter_bg: c(0xff, 0xff, 0xff),
                selection: ca(0x54, 0xae, 0xff, 0.4),
                mark_current: c(0xff, 0xf3, 0xc4),
                mark_open: c(0xff, 0xf9, 0xe3),
                mark_bar: c(0xd4, 0xa7, 0x2c),
                mark_past_bar: ca(0x82, 0x50, 0xdf, 0.5),
                scrollbar: ca(0x00, 0x00, 0x00, 0.25),
            }
        }
    }
}

/// A line of the file.
#[derive(Debug, Clone)]
pub struct Line {
    /// Text with tabs expanded (what is drawn).
    pub display: String,
}

pub fn lines_of(text: &str) -> Vec<Line> {
    text.lines().map(|l| Line { display: highlight::expand_tabs(l) }).collect()
}

/// How a range of lines stands out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MarkKind {
    /// Covered earlier: only a bar in front of the lines.
    Past,
    /// Covered earlier, and its chat is open.
    Open,
    /// What the walkthrough is at.
    Current,
}

/// A marked range of lines, as 0-based indices (`last` included).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mark {
    pub first: usize,
    pub last: usize,
    pub kind: MarkKind,
}

/// A position in the view: row index and display column (char cell).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct Pos {
    pub row: usize,
    pub col: usize,
}

pub struct CodeView<'a, Message> {
    rows: &'a [Line],
    spans: &'a [LineSpans],
    marks: Vec<Mark>,
    colors: Colors,
    font_size: f32,
    line_height: f32,
    font: Font,
    wrap: bool,
    /// (row, expanded) of each insert; its badge and body are children `2k` and `2k + 1`.
    inserts: Vec<(usize, bool)>,
    children: Vec<Element<'a, Message>>,
}

/// Widgets attached to a line.
pub struct Insert<'a, Message> {
    pub row: usize,
    /// Shown at the right end of the line.
    pub badge: Element<'a, Message>,
    /// Shown in a gap under the line while `expanded`.
    pub body: Element<'a, Message>,
    pub expanded: bool,
}

impl<'a, Message> CodeView<'a, Message> {
    pub fn new(rows: &'a [Line], spans: &'a [LineSpans], colors: Colors) -> Self {
        Self {
            rows,
            spans,
            marks: Vec::new(),
            colors,
            font_size: crate::fonts::code_size(),
            line_height: crate::fonts::code_line_height(crate::fonts::code_size()),
            font: crate::fonts::code(),
            wrap: false,
            inserts: Vec::new(),
            children: Vec::new(),
        }
    }

    /// Attaches widgets to lines; inserts whose line doesn't exist are ignored.
    pub fn inserts(mut self, inserts: impl IntoIterator<Item = Insert<'a, Message>>) -> Self {
        for i in inserts.into_iter().filter(|i| i.row < self.rows.len()) {
            self.inserts.push((i.row, i.expanded));
            self.children.push(i.badge);
            self.children.push(i.body);
        }
        self
    }

    /// Whether child `k` is shown: badges always, bodies while expanded.
    fn child_visible(&self, k: usize) -> bool {
        k.is_multiple_of(2) || self.inserts[k / 2].1
    }

    /// Overrides the font (defaults to the configured code font).
    pub fn font(mut self, font: Font, size: f32) -> Self {
        self.font = font;
        self.font_size = size;
        self.line_height = crate::fonts::code_line_height(size);
        self
    }

    /// Soft-wraps lines that don't fit the width instead of scrolling horizontally.
    pub fn wrap(mut self, wrap: bool) -> Self {
        self.wrap = wrap;
        self
    }

    pub fn marks(mut self, marks: impl IntoIterator<Item = Mark>) -> Self {
        self.marks.extend(marks);
        self
    }

    /// The most prominent mark on a line.
    fn mark_at(&self, row: usize) -> Option<MarkKind> {
        self.marks.iter().filter(|m| (m.first..=m.last).contains(&row)).map(|m| m.kind).max()
    }
}

#[derive(Debug, Default)]
struct State {
    anchor: Option<Pos>,
    head: Option<Pos>,
    dragging: bool,
    focused: bool,
    scroll_x: f32,
    char_width: f32,
    /// Font and size `char_width` was measured for.
    measured: Option<(Font, u32)>,
    modifiers: keyboard::Modifiers,
    last_click: Option<(Instant, Pos, u8)>,
    /// Offset of the cursor from the scrollbar thumb's left edge while it is dragged.
    hbar_grab: Option<f32>,
    /// Computed during layout; reused by update/draw so events stay O(1) in file size.
    metrics: Metrics,
    lines: Vec<VLine>,
    /// Gaps opened under rows for insert bodies, sorted by line.
    gaps: Vec<Gap>,
    /// `gap_prefix[k]`: total height of the first `k` gaps.
    gap_prefix: Vec<f32>,
}

/// Space under a visual line for an insert's body.
#[derive(Debug, Clone, Copy)]
struct Gap {
    /// The visual line the gap follows.
    after: usize,
    height: f32,
}

impl State {
    fn range(&self) -> Option<(Pos, Pos)> {
        let (a, h) = (self.anchor?, self.head?);
        if a == h { None } else { Some((a.min(h), a.max(h))) }
    }

    /// Top of visual line `li`, relative to the top of the view.
    fn line_top(&self, li: usize) -> f32 {
        let k = self.gaps.partition_point(|g| g.after < li);
        li as f32 * self.metrics.lh + self.gap_prefix.get(k).copied().unwrap_or(0.0)
    }

    /// Height of the lines and gaps (without the scrollbar).
    fn content_height(&self) -> f32 {
        self.lines.len() as f32 * self.metrics.lh + self.gap_prefix.last().copied().unwrap_or(0.0)
    }

    /// The visual line at or above `y` (relative to the top of the view; the first
    /// line above it), and whether `y` is on that line rather than in a gap after it.
    fn line_near(&self, y: f32) -> Option<(usize, bool)> {
        if self.lines.is_empty() {
            return None;
        }
        let (mut lo, mut hi) = (0, self.lines.len());
        while lo < hi {
            let mid = (lo + hi) / 2;
            if self.line_top(mid) <= y { lo = mid + 1 } else { hi = mid }
        }
        if lo == 0 {
            return Some((0, false));
        }
        let li = lo - 1;
        Some((li, y < self.line_top(li) + self.metrics.lh))
    }

    /// The visual line `y` is on, if it isn't above, below or between lines.
    fn line_at(&self, y: f32) -> Option<usize> {
        self.line_near(y).filter(|&(_, on)| on).map(|(li, _)| li)
    }
}

/// Vertical space around an insert's body in its gap.
const GAP_PAD: f32 = 8.0;

/// Geometry derived from the font metrics and contents.
#[derive(Debug, Clone, Copy, Default)]
struct Metrics {
    cw: f32,
    lh: f32,
    num_w: f32,
    code_x: f32,
    content_w: f32,
}

/// One visual line: a row, or a piece of a wrapped row.
#[derive(Debug, Clone, Copy)]
struct VLine {
    row: usize,
    /// Range of display columns of the row shown on this line.
    start: usize,
    end: usize,
    /// First line of its row: draws the line number.
    first: bool,
    /// Last line of its row: draws the selected newline cell.
    last: bool,
}

/// Splits a line into pieces of at most `width` columns, preferring to break
/// after a space in the second half of a piece.
fn wrap_points(chars: &[char], width: usize) -> Vec<(usize, usize)> {
    let len = chars.len();
    let mut out = Vec::new();
    let mut s = 0;
    while len - s > width {
        let hard = s + width;
        let e = (s + width / 2 + 1..=hard).rev().find(|&p| chars[p - 1] == ' ').unwrap_or(hard);
        out.push((s, e));
        s = e;
    }
    out.push((s, len));
    out
}

/// The horizontal scrollbar's geometry.
struct HBar {
    track: Rectangle,
    thumb: Rectangle,
    /// Maximum horizontal scroll offset.
    max: f32,
}

const PAD: f32 = 8.0;
const MARK_BAR_W: f32 = 3.0;
const SCROLLBAR_H: f32 = 10.0;

impl<Message> CodeView<'_, Message> {
    fn metrics(&self, cw: f32, width: f32) -> (Metrics, Vec<VLine>) {
        let digits = (self.rows.len().to_string().len()).max(3) as f32;
        let num_w = digits * cw + 2.0 * PAD;
        let code_x = num_w + PAD;
        let avail_cols = (((width - code_x - PAD) / cw).floor().max(20.0)) as usize;

        let mut lines = Vec::with_capacity(self.rows.len());
        let mut max_extent = 0;
        for (i, r) in self.rows.iter().enumerate() {
            let len = r.display.chars().count();
            max_extent = max_extent.max(len);
            if self.wrap && len > avail_cols {
                let chars: Vec<char> = r.display.chars().collect();
                let pieces = wrap_points(&chars, avail_cols);
                let n = pieces.len();
                for (k, (start, end)) in pieces.into_iter().enumerate() {
                    lines.push(VLine { row: i, start, end, first: k == 0, last: k == n - 1 });
                }
            } else {
                lines.push(VLine { row: i, start: 0, end: len, first: true, last: true });
            }
        }
        let content_w = if self.wrap { width } else { code_x + max_extent as f32 * cw + PAD };
        (Metrics { cw, lh: self.line_height, num_w, code_x, content_w }, lines)
    }

    /// The horizontal scrollbar, if the content is wider than the view. It sits under
    /// the last line, or sticks to the bottom of the visible area while that is below it.
    fn hbar(&self, state: &State, bounds: Rectangle, viewport: &Rectangle) -> Option<HBar> {
        let m = &state.metrics;
        if m.content_w <= bounds.width || m.cw <= 0.0 {
            return None;
        }
        let track_x = bounds.x + m.num_w;
        let track_w = (bounds.width - m.num_w).max(1.0);
        let natural_y = bounds.y + state.content_height() + 1.0;
        let sticky_y = viewport.y + viewport.height - SCROLLBAR_H - 1.0;
        let y = natural_y.min(sticky_y).max(bounds.y);
        let max = m.content_w - bounds.width;
        let thumb_w = (track_w * bounds.width / m.content_w).max(30.0).min(track_w);
        let thumb_x = track_x + (track_w - thumb_w) * (state.scroll_x / max).clamp(0.0, 1.0);
        Some(HBar {
            track: Rectangle::new(Point::new(track_x, y), Size::new(track_w, SCROLLBAR_H)),
            thumb: Rectangle::new(Point::new(thumb_x, y), Size::new(thumb_w, SCROLLBAR_H)),
            max,
        })
    }

    /// Scroll offset that puts the thumb's left edge at `thumb_x`.
    fn scroll_for_thumb(bar: &HBar, thumb_x: f32) -> f32 {
        let room = (bar.track.width - bar.thumb.width).max(1.0);
        ((thumb_x - bar.track.x) / room).clamp(0.0, 1.0) * bar.max
    }

    fn pos_at(&self, state: &State, m: &Metrics, bounds: Rectangle, p: Point) -> Pos {
        let Some((li, _)) = state.line_near(p.y - bounds.y) else { return Pos::default() };
        let l = state.lines[li];
        let x = p.x - bounds.x - m.code_x + state.scroll_x;
        let col = l.start + ((x / m.cw).round().max(0.0) as usize).min(l.end - l.start);
        Pos { row: l.row, col }
    }

    fn selected_text(&self, start: Pos, end: Pos) -> String {
        let mut out = String::new();
        for r in start.row..=end.row.min(self.rows.len().saturating_sub(1)) {
            let line: Vec<char> = self.rows[r].display.chars().collect();
            let s = if r == start.row { start.col.min(line.len()) } else { 0 };
            let e = if r == end.row { end.col.min(line.len()) } else { line.len() };
            out.extend(&line[s..e.max(s)]);
            if r != end.row {
                out.push('\n');
            }
        }
        out
    }

    fn word_at(&self, p: Pos) -> (Pos, Pos) {
        let line: Vec<char> = self.rows.get(p.row).map(|r| r.display.chars().collect()).unwrap_or_default();
        let is_word = |c: char| c.is_alphanumeric() || c == '_';
        let mut s = p.col.min(line.len());
        let mut e = s;
        if s < line.len() && is_word(line[s]) || s > 0 && is_word(line[s - 1]) {
            while s > 0 && is_word(line[s - 1]) {
                s -= 1;
            }
            while e < line.len() && is_word(line[e]) {
                e += 1;
            }
        } else if s < line.len() {
            e = s + 1;
        }
        (Pos { row: p.row, col: s }, Pos { row: p.row, col: e })
    }
}

impl<Message> Widget<Message, Theme, Renderer> for CodeView<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }

    fn children(&self) -> Vec<Tree> {
        self.children.iter().map(Tree::new).collect()
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(&self.children);
    }

    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Shrink)
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) -> layout::Node {
        let state = tree.state.downcast_mut::<State>();
        let key = (self.font, (self.font_size * 100.0) as u32);
        if state.measured != Some(key) {
            state.measured = Some(key);
            let p = Paragraph::with_text(text::Text {
                content: "0123456789012345678901234567890123456789",
                bounds: Size::INFINITE,
                size: Pixels(self.font_size),
                line_height: text::LineHeight::Absolute(Pixels(self.line_height)),
                font: self.font,
                align_x: text::Alignment::Left,
                align_y: alignment::Vertical::Top,
                shaping: text::Shaping::Basic,
                wrapping: text::Wrapping::None,
            });
            state.char_width = (p.min_width() / 40.0).max(1.0);
        }
        let width = limits.max().width;
        let width = if width.is_finite() { width } else { 1200.0 };
        let (m, lines) = self.metrics(state.char_width, width);
        state.metrics = m;
        state.lines = lines;

        // Bodies of expanded inserts go in gaps under the last line of their row,
        // indented past the line number gutter.
        let last_line = |lines: &[VLine], row: usize| lines.partition_point(|l| l.row <= row).saturating_sub(1);
        let body_x = m.num_w + PAD;
        let body_limits = layout::Limits::new(Size::ZERO, Size::new((width - body_x - 2.0 * PAD).max(100.0), f32::INFINITY));
        let mut nodes: Vec<layout::Node> = Vec::with_capacity(self.children.len());
        let mut gaps: Vec<(Gap, usize)> = Vec::new(); // (gap, child index of the body)
        for (k, (child, child_tree)) in self.children.iter_mut().zip(&mut tree.children).enumerate() {
            let (row, expanded) = self.inserts[k / 2];
            if k % 2 == 1 && expanded {
                let node = child.as_widget_mut().layout(child_tree, renderer, &body_limits);
                gaps.push((Gap { after: last_line(&state.lines, row), height: node.size().height + 2.0 * GAP_PAD }, k));
                nodes.push(node);
            } else if k % 2 == 0 {
                nodes.push(child.as_widget_mut().layout(child_tree, renderer, &layout::Limits::new(Size::ZERO, Size::new(width, m.lh * 2.0))));
            } else {
                nodes.push(layout::Node::new(Size::ZERO));
            }
        }
        gaps.sort_by_key(|(g, _)| g.after);
        state.gap_prefix = std::iter::once(0.0)
            .chain(gaps.iter().scan(0.0, |sum, (g, _)| {
                *sum += g.height;
                Some(*sum)
            }))
            .collect();
        state.gaps = gaps.iter().map(|(g, _)| *g).collect();
        for (idx, (g, k)) in gaps.iter().enumerate() {
            let top = (g.after + 1) as f32 * m.lh + state.gap_prefix[idx];
            nodes[*k] = std::mem::take(&mut nodes[*k]).move_to(Point::new(body_x, top + GAP_PAD));
        }
        // Badges sit at the right end of the first line of their row.
        for (k, node) in nodes.iter_mut().enumerate().filter(|(k, _)| k % 2 == 0) {
            let first_line = state.lines.partition_point(|l| l.row < self.inserts[k / 2].0);
            let size = node.size();
            let y = state.line_top(first_line) + (m.lh - size.height) / 2.0;
            *node = std::mem::take(node).move_to(Point::new(width - size.width - PAD, y));
        }

        let mut height = state.content_height();
        if m.content_w > width {
            height += SCROLLBAR_H + 2.0;
        }
        state.scroll_x = state.scroll_x.min((m.content_w - width).max(0.0));
        layout::Node::with_children(Size::new(width, height), nodes)
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();
        if tree.state.downcast_ref::<State>().metrics.cw <= 0.0 {
            return;
        }

        // Inserted widgets get events first.
        let mut over_child = false;
        for (k, ((child, child_tree), child_layout)) in self.children.iter_mut().zip(&mut tree.children).zip(layout.children()).enumerate() {
            if k % 2 == 1 && !self.inserts[k / 2].1 {
                continue;
            }
            over_child |= cursor.is_over(child_layout.bounds());
            child.as_widget_mut().update(child_tree, event, child_layout, cursor, renderer, clipboard, shell, viewport);
        }
        let state = tree.state.downcast_mut::<State>();
        let m = state.metrics;
        if shell.is_event_captured() {
            // A click into an inserted widget still takes the focus away from the code.
            if matches!(event, Event::Mouse(mouse::Event::ButtonPressed(_))) && (state.anchor.is_some() || state.focused) {
                state.anchor = None;
                state.head = None;
                state.focused = false;
                shell.request_redraw();
            }
            return;
        }

        // Scrollbar: dragging the thumb, or clicking the track to jump there. It may
        // overlap code lines while it sticks to the bottom of the viewport, so it is
        // handled first.
        let bar = self.hbar(state, bounds, viewport);
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                if let Some(bar) = &bar
                    && let Some(p) = cursor.position_over(bar.track)
                {
                    let grab = if bar.thumb.contains(p) { p.x - bar.thumb.x } else { bar.thumb.width / 2.0 };
                    state.scroll_x = Self::scroll_for_thumb(bar, p.x - grab);
                    state.hbar_grab = Some(grab);
                    shell.capture_event();
                    shell.request_redraw();
                    return;
                }
            }
            Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                if let (Some(bar), Some(grab)) = (&bar, state.hbar_grab) {
                    if let Some(p) = cursor.land().position() {
                        state.scroll_x = Self::scroll_for_thumb(bar, p.x - grab);
                    }
                    shell.capture_event();
                    shell.request_redraw();
                    return;
                }
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) if state.hbar_grab.is_some() => {
                state.hbar_grab = None;
                shell.capture_event();
                shell.request_redraw();
                return;
            }
            _ => {}
        }

        let over_code = cursor
            .position_over(bounds)
            .filter(|p| !over_child && state.line_at(p.y - bounds.y).is_some())
            .filter(|p| !bar.as_ref().is_some_and(|b| b.track.contains(*p)));

        match event {
            Event::Keyboard(keyboard::Event::ModifiersChanged(mods)) => state.modifiers = *mods,
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                if let Some(p) = over_code {
                    let pos = self.pos_at(state, &m, bounds, p);
                    let now = Instant::now();
                    let clicks = match state.last_click {
                        Some((t, prev, n)) if now - t < Duration::from_millis(400) && prev.row == pos.row => n % 3 + 1,
                        _ => 1,
                    };
                    state.last_click = Some((now, pos, clicks));
                    match clicks {
                        2 => {
                            let (s, e) = self.word_at(pos);
                            state.anchor = Some(s);
                            state.head = Some(e);
                            state.dragging = false;
                        }
                        3 => {
                            let len = self.rows[pos.row].display.chars().count();
                            state.anchor = Some(Pos { row: pos.row, col: 0 });
                            state.head = Some(Pos { row: pos.row, col: len });
                            state.dragging = false;
                        }
                        _ => {
                            if state.modifiers.shift() && state.anchor.is_some() {
                                state.head = Some(pos);
                            } else {
                                state.anchor = Some(pos);
                                state.head = Some(pos);
                            }
                            state.dragging = true;
                        }
                    }
                    state.focused = true;
                    shell.capture_event();
                    shell.request_redraw();
                } else if state.anchor.is_some() || state.focused {
                    state.anchor = None;
                    state.head = None;
                    state.focused = false;
                    shell.request_redraw();
                }
            }
            // The event's position is in window coordinates; the cursor has been
            // adjusted for the scroll offset of the page this view sits in.
            Event::Mouse(mouse::Event::CursorMoved { .. }) if state.dragging => {
                if let Some(p) = cursor.land().position() {
                    state.head = Some(self.pos_at(state, &m, bounds, p));
                    shell.request_redraw();
                }
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) if state.dragging => {
                state.dragging = false;
            }
            Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
                if cursor.is_over(bounds) && m.content_w > bounds.width {
                    let (dx, dy) = match delta {
                        mouse::ScrollDelta::Lines { x, y } => (x * 3.0 * m.cw, y * 3.0 * m.cw),
                        mouse::ScrollDelta::Pixels { x, y } => (*x, *y),
                    };
                    let dx = if state.modifiers.shift() && dx == 0.0 { dy } else { dx };
                    if dx != 0.0 {
                        let max = (m.content_w - bounds.width).max(0.0);
                        state.scroll_x = (state.scroll_x - dx).clamp(0.0, max);
                        shell.capture_event();
                        shell.request_redraw();
                    }
                }
            }
            Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) if state.focused => {
                if modifiers.command() && matches!(key.as_ref(), keyboard::Key::Character("c")) {
                    if let Some((s, e)) = state.range() {
                        clipboard.write(clipboard::Kind::Standard, self.selected_text(s, e));
                        shell.capture_event();
                    }
                } else if modifiers.command() && matches!(key.as_ref(), keyboard::Key::Character("a")) {
                    if let Some(last) = self.rows.len().checked_sub(1) {
                        state.anchor = Some(Pos::default());
                        state.head = Some(Pos { row: last, col: self.rows[last].display.chars().count() });
                        shell.capture_event();
                        shell.request_redraw();
                    }
                } else if matches!(key.as_ref(), keyboard::Key::Named(keyboard::key::Named::Escape)) {
                    state.anchor = None;
                    state.head = None;
                    shell.request_redraw();
                }
            }
            _ => {}
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        let state = tree.state.downcast_ref::<State>();
        let b = layout.bounds();
        let m = state.metrics;
        for (k, ((child, child_tree), child_layout)) in self.children.iter().zip(&tree.children).zip(layout.children()).enumerate() {
            if self.child_visible(k) && cursor.is_over(child_layout.bounds()) {
                return child.as_widget().mouse_interaction(child_tree, child_layout, cursor, viewport, renderer);
            }
        }
        if state.hbar_grab.is_some() {
            return mouse::Interaction::Grabbing;
        }
        if let Some(bar) = self.hbar(state, b, viewport)
            && cursor.is_over(bar.track)
        {
            return mouse::Interaction::Grab;
        }
        match cursor.position_over(b) {
            Some(p) if p.x >= b.x + m.code_x && state.line_at(p.y - b.y).is_some() => mouse::Interaction::Text,
            _ => mouse::Interaction::None,
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_ref::<State>();
        let bounds = layout.bounds();
        let Some(visible) = bounds.intersection(viewport) else { return };
        let m = state.metrics;
        let c = &self.colors;
        let first = state.line_near(visible.y - bounds.y).map_or(0, |(li, _)| li);
        let last = state.line_near(visible.y + visible.height - bounds.y).map_or(0, |(li, _)| li + 1);
        let selection = state.range();

        let quad = |r: &mut Renderer, rect: Rectangle, color: Color| {
            r.fill_quad(renderer::Quad { bounds: rect, ..Default::default() }, Background::Color(color));
        };
        let txt = |content: String, font: Font| text::Text {
            shaping: if content.is_ascii() { text::Shaping::Basic } else { text::Shaping::Advanced },
            content,
            bounds: Size::new(f32::INFINITY, m.lh),
            size: Pixels(self.font_size),
            line_height: text::LineHeight::Absolute(Pixels(m.lh)),
            font,
            align_x: text::Alignment::Left,
            align_y: alignment::Vertical::Top,
            wrapping: text::Wrapping::None,
        };

        // The gutter continues next to the gaps for insert bodies.
        for (idx, g) in state.gaps.iter().enumerate() {
            let top = bounds.y + (g.after + 1) as f32 * m.lh + state.gap_prefix[idx];
            let rect = Rectangle::new(Point::new(bounds.x, top), Size::new(m.num_w, g.height));
            if rect.intersects(&visible) {
                quad(renderer, rect, c.gutter_bg);
            }
        }

        // Gutter, marks and line numbers.
        for li in first..last {
            let l = state.lines[li];
            let y = bounds.y + state.line_top(li);
            quad(renderer, Rectangle::new(Point::new(bounds.x, y), Size::new(m.num_w, m.lh)), c.gutter_bg);
            let (bg, bar) = match self.mark_at(l.row) {
                Some(MarkKind::Current) => (Some(c.mark_current), Some(c.mark_bar)),
                Some(MarkKind::Open) => (Some(c.mark_open), Some(c.mark_bar)),
                Some(MarkKind::Past) => (None, Some(c.mark_past_bar)),
                None => (None, None),
            };
            if let Some(bg) = bg {
                quad(renderer, Rectangle::new(Point::new(bounds.x, y), Size::new(bounds.width, m.lh)), bg);
            }
            if let Some(bar) = bar {
                quad(renderer, Rectangle::new(Point::new(bounds.x + m.num_w - MARK_BAR_W, y), Size::new(MARK_BAR_W, m.lh)), bar);
            }
            if l.first {
                let s = (l.row + 1).to_string();
                let x = bounds.x + m.num_w - PAD - s.len() as f32 * m.cw;
                renderer.fill_text(txt(s, self.font), Point::new(x, y), c.muted, visible);
            }
        }

        // Code area (clipped, horizontally scrolled).
        let code_clip = Rectangle {
            x: bounds.x + m.num_w,
            y: visible.y,
            width: (bounds.width - m.num_w).max(0.0),
            height: visible.height,
        };
        let Some(code_clip) = code_clip.intersection(&visible) else { return };
        renderer.with_layer(code_clip, |renderer| {
            let base_x = bounds.x + m.code_x - state.scroll_x;
            for li in first..last {
                let l = state.lines[li];
                let i = l.row;
                let row = &self.rows[i];
                let y = bounds.y + state.line_top(li);
                let len = row.display.chars().count();

                if let Some((s, e)) = selection
                    && (s.row..=e.row).contains(&i) {
                        let sc = if i == s.row { s.col } else { 0 };
                        let ec = if i == e.row { e.col } else { len + 1 };
                        // The cell after the last text line stands for the newline.
                        let line_end = if l.last { l.end + 1 } else { l.end };
                        let (a, b) = (sc.max(l.start), ec.min(line_end));
                        if b > a {
                            quad(
                                renderer,
                                Rectangle::new(Point::new(base_x + (a - l.start) as f32 * m.cw, y), Size::new((b - a) as f32 * m.cw, m.lh)),
                                c.selection,
                            );
                        }
                    }

                let whole = l.start == 0 && l.end == len;
                let piece = |from: usize, to: usize| -> String { row.display.chars().skip(from).take(to.saturating_sub(from)).collect() };
                let spans = self.spans.get(i).map(Vec::as_slice).unwrap_or(&[]);
                if l.start == l.end {
                    // Nothing to draw (an empty line).
                } else if spans.is_empty() {
                    let content = if whole { row.display.clone() } else { piece(l.start, l.end) };
                    renderer.fill_text(txt(content, self.font), Point::new(base_x, y), c.text, code_clip);
                } else {
                    let chars: Vec<char> = row.display.chars().collect();
                    // Merge adjacent spans that share a style to reduce draw calls.
                    let mut k = 0;
                    while k < spans.len() {
                        let sp = &spans[k];
                        let start = sp.range.start.min(chars.len());
                        let mut end = sp.range.end.min(chars.len());
                        let mut j = k + 1;
                        while j < spans.len()
                            && spans[j].color == sp.color
                            && spans[j].bold == sp.bold
                            && spans[j].italic == sp.italic
                            && spans[j].range.start == end
                        {
                            end = spans[j].range.end.min(chars.len());
                            j += 1;
                        }
                        k = j;
                        // Only the part of the span on this visual line.
                        let (start, end) = (start.max(l.start), end.min(l.end));
                        if start >= l.end {
                            break;
                        }
                        if end <= start {
                            continue;
                        }
                        let text: String = chars[start..end].iter().collect();
                        if text.trim().is_empty() {
                            continue;
                        }
                        let x = base_x + (start - l.start) as f32 * m.cw;
                        if x > code_clip.x + code_clip.width {
                            break;
                        }
                        let font = Font {
                            weight: if sp.bold { Weight::Bold } else { Weight::Normal },
                            style: if sp.italic { FontStyle::Italic } else { FontStyle::Normal },
                            ..self.font
                        };
                        renderer.fill_text(txt(text, font), Point::new(x, y), sp.color, code_clip);
                    }
                }
            }
        });

        // Inserted widgets.
        for (k, ((child, child_tree), child_layout)) in self.children.iter().zip(&tree.children).zip(layout.children()).enumerate() {
            if self.child_visible(k) && child_layout.bounds().intersects(&visible) {
                child.as_widget().draw(child_tree, renderer, theme, style, child_layout, cursor, &visible);
            }
        }

        // Horizontal scrollbar, in its own layer so it covers the code while it sticks
        // to the bottom of the viewport.
        if let Some(bar) = self.hbar(state, bounds, viewport) {
            renderer.with_layer(visible, |renderer| {
                quad(renderer, bar.track, c.gutter_bg);
                let active = state.hbar_grab.is_some() || cursor.is_over(bar.track);
                let color = if active { Color { a: (c.scrollbar.a * 2.0).min(1.0), ..c.scrollbar } } else { c.scrollbar };
                renderer.fill_quad(
                    renderer::Quad {
                        bounds: Rectangle::new(Point::new(bar.thumb.x, bar.thumb.y + 2.0), Size::new(bar.thumb.width, bar.thumb.height - 4.0)),
                        border: Border { radius: 3.0.into(), ..Default::default() },
                        ..Default::default()
                    },
                    Background::Color(color),
                );
            });
        }
    }

    fn operate(&mut self, tree: &mut Tree, layout: Layout<'_>, renderer: &Renderer, operation: &mut dyn widget::Operation) {
        let visible: Vec<bool> = (0..self.children.len()).map(|k| self.child_visible(k)).collect();
        operation.traverse(&mut |operation| {
            for (k, ((child, child_tree), child_layout)) in self.children.iter_mut().zip(&mut tree.children).zip(layout.children()).enumerate() {
                if visible[k] {
                    child.as_widget_mut().operate(child_tree, child_layout, renderer, operation);
                }
            }
        });
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, Renderer>> {
        let visible: Vec<bool> = (0..self.children.len()).map(|k| self.child_visible(k)).collect();
        let overlays: Vec<_> = self
            .children
            .iter_mut()
            .zip(&mut tree.children)
            .zip(layout.children())
            .enumerate()
            .filter(|(k, _)| visible[*k])
            .filter_map(|(_, ((child, child_tree), child_layout))| {
                child.as_widget_mut().overlay(child_tree, child_layout, renderer, viewport, translation)
            })
            .collect();
        (!overlays.is_empty()).then(|| overlay::Group::with_children(overlays).overlay())
    }
}

impl<'a, Message: 'a> From<CodeView<'a, Message>> for Element<'a, Message, Theme, Renderer> {
    fn from(v: CodeView<'a, Message>) -> Self {
        Element::new(v)
    }
}

#[cfg(test)]
mod tests {
    use super::{CodeView, Colors, Mark, MarkKind, lines_of, wrap_points};

    fn pieces(s: &str, width: usize) -> Vec<String> {
        let chars: Vec<char> = s.chars().collect();
        wrap_points(&chars, width).into_iter().map(|(a, b)| chars[a..b].iter().collect()).collect()
    }

    #[test]
    fn the_most_prominent_mark_wins() {
        let lines = lines_of("a\nb\nc\n");
        let view: CodeView<'_, ()> = CodeView::new(&lines, &[], Colors::new(false)).marks([
            Mark { first: 0, last: 1, kind: MarkKind::Past },
            Mark { first: 1, last: 1, kind: MarkKind::Current },
        ]);
        assert_eq!(view.mark_at(0), Some(MarkKind::Past));
        assert_eq!(view.mark_at(1), Some(MarkKind::Current));
        assert_eq!(view.mark_at(2), None);
    }

    #[test]
    fn tabs_are_expanded() {
        assert_eq!(lines_of("a\tb").first().unwrap().display, "a   b");
    }

    #[test]
    fn short_and_empty_lines_are_one_piece() {
        assert_eq!(pieces("", 10), [""]);
        assert_eq!(pieces("0123456789", 10), ["0123456789"]);
    }

    #[test]
    fn breaks_after_a_space_in_the_second_half() {
        assert_eq!(pieces("let x = foo(bar, baz);", 12), ["let x = ", "foo(bar, ", "baz);"]);
    }

    #[test]
    fn breaks_hard_without_a_late_space() {
        assert_eq!(pieces("a abcdefghijklmnop", 8), ["a abcdef", "ghijklmn", "op"]);
    }

    #[test]
    fn pieces_cover_the_line_exactly() {
        let s = "fn compute(argument_one: u32, argument_two: &str) -> Result<(), Error> { todo!() }";
        for w in 20..40 {
            let chars: Vec<char> = s.chars().collect();
            let p = wrap_points(&chars, w);
            assert_eq!(p.first().unwrap().0, 0);
            assert_eq!(p.last().unwrap().1, chars.len());
            assert!(p.windows(2).all(|w| w[0].1 == w[1].0));
            assert!(p.iter().all(|(a, b)| b - a <= w && b > a));
        }
    }
}
