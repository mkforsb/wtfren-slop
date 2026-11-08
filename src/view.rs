//! All screens and overlays.

use crate::app::*;
use crate::claude;
use crate::fonts;
use crate::code_view::{CodeView, Colors, Insert, Mark, MarkKind};
use crate::selectable_text::SelectableText;
use crate::style::{self, small};
use crate::tracker::Tracker;
use iced::widget::text_editor::{Binding, KeyPress};
use iced::widget::{
    Space, button, center, column, combo_box, container, markdown, mouse_area, opaque, row, scrollable, slider, stack, text,
    text_editor, text_input, toggler, tooltip,
};
use iced::{Alignment, Element, Fill, Font, Length, Padding, Theme};
use std::collections::BTreeMap;

type El<'a> = Element<'a, Message>;

/// What markdown views need: the markdown settings, and the text of the most
/// recent copy so its button can say "Copied!" for a moment.
#[derive(Clone, Copy)]
struct Md<'a> {
    settings: markdown::Settings,
    copied: Option<&'a str>,
}

impl Md<'_> {
    fn copy_label(&self, text: &str) -> &'static str {
        if self.copied == Some(text) { "Copied!" } else { "Copy" }
    }
}

/// Markdown viewer with selectable text and GitHub-like code blocks (subtle
/// background, copy button).
struct MdViewer<'a> {
    md: Md<'a>,
}

impl<'a> markdown::Viewer<'a, Message> for MdViewer<'a> {
    fn on_link_click(url: markdown::Uri) -> Message {
        Message::LinkClicked(url)
    }

    fn paragraph(&self, settings: markdown::Settings, text: &markdown::Text) -> El<'a> {
        SelectableText::new(text.spans(settings.style).iter().cloned())
            .size(settings.text_size)
            .on_link_click(Message::LinkClicked)
            .into()
    }

    fn heading(
        &self,
        settings: markdown::Settings,
        level: &'a markdown::HeadingLevel,
        text: &'a markdown::Text,
        index: usize,
    ) -> El<'a> {
        use markdown::HeadingLevel as H;
        let size = match level {
            H::H1 => settings.h1_size,
            H::H2 => settings.h2_size,
            H::H3 => settings.h3_size,
            H::H4 => settings.h4_size,
            H::H5 => settings.h5_size,
            H::H6 => settings.h6_size,
        };
        let top = if index > 0 { settings.text_size.0 / 2.0 } else { 0.0 };
        container(SelectableText::new(text.spans(settings.style).iter().cloned()).size(size).on_link_click(Message::LinkClicked))
            .padding(Padding { top, ..Padding::ZERO })
            .into()
    }

    fn code_block(
        &self,
        settings: markdown::Settings,
        _language: Option<&'a str>,
        code: &'a str,
        lines: &'a [markdown::Text],
    ) -> El<'a> {
        // One widget for the whole block, so a selection can span several lines.
        let mut spans = Vec::new();
        for (i, line) in lines.iter().enumerate() {
            if i > 0 {
                spans.push(iced::advanced::text::Span::new("\n"));
            }
            spans.extend(line.spans(settings.style).iter().cloned());
        }
        let body = SelectableText::new(spans).on_link_click(Message::LinkClicked).font(fonts::code()).size(13);
        container(
            column![
                row![
                    Space::new().width(Fill),
                    button(small(self.md.copy_label(code))).padding([0, 6]).style(style::flat_button).on_press(Message::CopyText(code.to_string())),
                ],
                scrollable(container(body).padding(Padding { bottom: 8.0, ..Padding::ZERO }))
                    .direction(scrollable::Direction::Horizontal(scrollable::Scrollbar::default().width(4).scroller_width(4))),
            ]
            .spacing(2),
        )
        .padding([6, 10])
        .width(Fill)
        .style(|t: &Theme| {
            let mut s = style::card(t);
            s.background = Some(iced::Background::Color(style::subtle_bg(t)));
            s
        })
        .into()
    }
}

fn md_view<'a>(items: &'a [markdown::Item], md: Md<'a>) -> El<'a> {
    markdown::view_with(items, md.settings, &MdViewer { md })
}

impl App {
    pub fn view(&self) -> El<'_> {
        let theme = self.theme();
        let base: El = match &self.screen {
            Screen::DirSelect(s) => dir_select_view(s, &self.settings.recent_dirs),
            Screen::Walkthrough(w) => walkthrough_view(w, self, &theme),
        };
        let mut layers = vec![base];

        if let Screen::Walkthrough(w) = &self.screen
            && let Some(p) = w.claude.permissions.front()
        {
            layers.push(modal(permission_view(p, w.claude.permissions.len()), None));
        }
        if let Some(ed) = &self.settings_editor {
            layers.push(modal(settings_view(ed, &self.settings.ui_font), Some(Message::CloseSettings)));
        }
        if let Some(err) = &self.modal_error {
            layers.push(modal(error_modal_view(err), Some(Message::DismissModal)));
        }
        stack(layers).width(Fill).height(Fill).into()
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn modal<'a>(content: El<'a>, on_blur: Option<Message>) -> El<'a> {
    opaque(mouse_area(center(opaque(content)).style(style::backdrop)).on_press(on_blur.unwrap_or(Message::Noop)))
}

fn btn<'a>(label: impl text::IntoFragment<'a>) -> button::Button<'a, Message> {
    button(text(label).size(13)).padding([5, 12]).style(style::default_button)
}

fn primary_btn<'a>(label: impl text::IntoFragment<'a>) -> button::Button<'a, Message> {
    button(text(label).size(13)).padding([5, 12]).style(button::primary)
}

fn icon_btn<'a>(label: impl text::IntoFragment<'a>, tip: &'a str, msg: Message) -> El<'a> {
    tooltip(
        button(text(label).size(14)).padding([3, 8]).style(style::flat_button).on_press(msg),
        container(text(tip).size(12)).padding([4, 8]).style(style::menu),
        tooltip::Position::Bottom,
    )
    .into()
}

fn errors_view(errors: &[String]) -> El<'_> {
    column(errors.iter().enumerate().map(|(i, e)| style::error_box(e, Some(Message::DismissError(i)))))
        .spacing(8)
        .into()
}

fn settings_gear<'a>() -> El<'a> {
    icon_btn("⚙", "Settings", Message::OpenSettings)
}

// ---------------------------------------------------------------------------
// Directory select
// ---------------------------------------------------------------------------

fn dir_select_view<'a>(s: &'a DirSelect, recent: &'a [std::path::PathBuf]) -> El<'a> {
    let header = row![text("Codebase Walkthrough").size(28), Space::new().width(Fill), settings_gear()].align_y(Alignment::Center);
    let picker = row![
        primary_btn(if s.busy { "Opening…" } else { "Choose directory…" }).on_press_maybe((!s.busy).then_some(Message::PickDir)),
        text("or").size(13),
        text_input("/path/to/directory", &s.path_input)
            .on_input(Message::DirPathChanged)
            .on_submit(Message::OpenDirPath)
            .padding(6)
            .size(13),
        btn("Open").on_press_maybe((!s.busy).then_some(Message::OpenDirPath)),
    ]
    .spacing(10)
    .align_y(Alignment::Center);

    let mut content = column![
        header,
        text("Select a local directory and let Claude walk you through the code in it. Git repositories work best, since Claude can use their history.").size(14),
        picker,
        errors_view(&s.errors),
    ]
    .spacing(16)
    .max_width(820);

    if !recent.is_empty() {
        let list = column(recent.iter().map(|p| {
            button(row![text("›").size(14), text(p.display().to_string()).size(13)].spacing(8))
                .width(Fill)
                .padding([6, 10])
                .style(style::flat_button)
                .on_press_maybe((!s.busy).then(|| Message::OpenRecent(p.clone())))
                .into()
        }))
        .spacing(2);
        content = content.push(
            container(column![text("Recently used").size(14), list].spacing(8))
                .padding(12)
                .width(Fill)
                .style(style::card),
        );
    }
    container(content).padding(40).center_x(Fill).into()
}

// ---------------------------------------------------------------------------
// Walkthrough
// ---------------------------------------------------------------------------

fn muted<'a>(s: impl text::IntoFragment<'a>) -> text::Text<'a> {
    small(s).style(|t: &Theme| text::Style { color: Some(style::muted(t)) })
}

fn walkthrough_view<'a>(w: &'a Walkthrough, app: &'a App, theme: &Theme) -> El<'a> {
    let name = w.dir.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| w.dir.path.display().to_string());
    let top = container(
        row![
            btn("← Directories").on_press(Message::BackToDirSelect),
            column![text(name).size(17).font(fonts::ui_weight(iced::font::Weight::Semibold)), muted(w.dir.path.display().to_string())]
                .spacing(2)
                .width(Fill),
            text(w.progress()).size(12),
            icon_btn("⊞", "Expand all files", Message::ExpandAll),
            icon_btn("⊟", "Collapse all files", Message::CollapseAll),
            icon_btn(if app.settings.show_tree { "◧" } else { "▢" }, "Toggle file tree", Message::ToggleTree),
            icon_btn(if w.show_chat { "◨" } else { "▢" }, "Toggle Claude chat panel", Message::ToggleChat),
            settings_gear(),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    )
    .padding([8, 12])
    .width(Fill)
    .style(style::bar);

    let colors = Colors::new(style::is_dark(theme));
    let md = Md { settings: style::markdown_settings(theme), copied: app.copied() };

    // The bottom padding makes room for scrolling the last file to the top of the view.
    let mut page = column![].spacing(16).padding(Padding { top: 16.0, right: 20.0, bottom: app.window_size.height, left: 16.0 });
    if !w.errors.is_empty() {
        page = page.push(errors_view(&w.errors));
    }
    let mut sections = Vec::new();
    for (i, f) in w.files.iter().enumerate() {
        if w.is_visible(i, f) {
            sections.push((i, iced::widget::Id::from(file_section_id(i))));
            page = page.push(file_section(w, i, f, colors, md, app.settings.wrap_lines));
        }
    }
    if w.files.is_empty() {
        page = page.push(empty_page_view(w));
    } else if sections.is_empty() {
        page = page.push(text("No files match the current filter.").size(14));
    }
    let main = Tracker::new(scrollable(page).id(PAGE_SCROLL_ID).width(Fill).height(Fill), PAGE_SCROLL_ID, sections, Message::LookingAt);

    let mut body = row![].height(Fill);
    if app.settings.show_tree {
        body = body.push(file_tree_view(w));
    }
    body = body.push(main);
    if w.show_chat {
        body = body.push(general_chat_view(w, md));
    }

    column![top, body, status_bar(w)].into()
}

/// The page before the first file arrives.
fn empty_page_view(w: &Walkthrough) -> El<'_> {
    let message = match &w.phase {
        Phase::Orienting => "Claude is getting oriented in the codebase. Follow along in the chat panel.",
        Phase::Planning => "Claude is planning the order of the walkthrough.",
        Phase::Walking(_) => "Opening the first file…",
        Phase::Chat | Phase::Finished => "Files appear here one at a time as the walkthrough progresses.",
    };
    container(muted(message)).padding(24).center_x(Fill).into()
}

fn banner<'a>(content: impl Into<El<'a>>) -> El<'a> {
    container(content).padding([6, 12]).width(Fill).style(style::info_box_style).into()
}

fn file_section<'a>(w: &'a Walkthrough, i: usize, f: &'a FileEntry, colors: Colors, md: Md<'a>, wrap_default: bool) -> El<'a> {
    let wrap = f.wraps(wrap_default);
    let header = row![
        button(text(if f.collapsed { "▸" } else { "▾" }).size(14)).padding([2, 6]).style(style::flat_button).on_press(Message::ToggleCollapsed(i)),
        text(&f.path).size(13).font(fonts::code()),
        if md.copied == Some(f.path.as_str()) {
            icon_btn("✓", "Copied!", Message::CopyText(f.path.clone()))
        } else {
            icon_btn("⧉", "Copy path", Message::CopyText(f.path.clone()))
        },
        Space::new().width(Fill),
        muted(format!("{} lines", f.code.lines.len())),
        tooltip(
            button(text("↩ Wrap").size(12)).padding([3, 8]).style(style::toggle_button(wrap)).on_press(Message::ToggleWrap(i)),
            container(text(if wrap { "Scroll long lines horizontally" } else { "Wrap long lines" }).size(12)).padding([4, 8]).style(style::menu),
            tooltip::Position::Bottom,
        ),
    ]
    .spacing(6)
    .align_y(Alignment::Center);

    let mut col = column![container(header).padding([4, 8]).width(Fill).style(style::card_header)];

    if !f.collapsed {
        // The banners share one always-present column, so the code view keeps its position
        // in the widget tree (and with it, selection, scroll and focus).
        let mut banners = column![];
        if !f.why.is_empty() {
            banners = banners.push(container(muted(&f.why)).padding([6, 12]));
        }
        if w.file == Some(i)
            && let Some(walk) = w.walk()
        {
            if walk.waiting {
                banners = banners.push(banner(text(format!("✦ Claude is preparing the next element… {}", w.claude.activity)).size(12)));
            }
            if let Some(e) = &walk.failed {
                banners = banners.push(banner(
                    row![
                        text(format!("The walkthrough stopped: {e}")).size(12).width(Fill),
                        button(text("↻ Retry").size(12)).padding([2, 8]).style(style::flat_button).on_press(Message::RetryElement),
                    ]
                    .align_y(Alignment::Center),
                ));
            }
        }
        col = col.push(banners);
        let inserts = f.elements.iter().enumerate().map(|(e, el)| Insert {
            row: el.end as usize - 1,
            badge: element_badge(w, i, e),
            body: element_chat_view(w, i, e, el, md),
            expanded: el.expanded,
        });
        let marks = f.elements.iter().enumerate().map(|(e, el)| Mark {
            first: el.start as usize - 1,
            last: el.end as usize - 1,
            kind: match (w.active == Some((i, e)), el.expanded) {
                (true, _) => MarkKind::Current,
                (false, true) => MarkKind::Open,
                (false, false) => MarkKind::Past,
            },
        });
        col = col.push(
            container(CodeView::new(&f.code.lines, &f.code.spans, colors).wrap(wrap).inserts(inserts).marks(marks))
                .padding(Padding { top: 0.0, right: 1.0, bottom: 4.0, left: 1.0 }),
        );
    }
    container(col).id(file_section_id(i)).width(Fill).style(style::card).into()
}

/// The badge at the end of an element's selection. Clicking it shows or hides the chat.
fn element_badge<'a>(w: &'a Walkthrough, i: usize, e: usize) -> El<'a> {
    let active = w.active == Some((i, e));
    let expanded = w.files[i].elements[e].expanded;
    tooltip(
        button(text(format!("✦ {}", e + 1)).size(12))
            .padding([1, 6])
            .style(|t: &Theme, status| {
                let mut s = style::flat_button(t, status);
                if s.background.is_none() {
                    s.background = Some(iced::Background::Color(iced::Color { a: 0.85, ..t.palette().background }));
                }
                s.border.radius = 11.0.into();
                s
            })
            .on_press_maybe((!active).then_some(Message::ToggleElement(i, e))),
        container(text(if active { "The current element" } else if expanded { "Hide this explanation" } else { "Show this explanation" }).size(12))
            .padding([4, 8])
            .style(style::menu),
        tooltip::Position::Left,
    )
    .into()
}

fn chat_key_binding<'a>(send: Message) -> impl Fn(KeyPress) -> Option<Binding<Message>> + 'a {
    move |kp: KeyPress| {
        if let iced::keyboard::Key::Named(iced::keyboard::key::Named::Enter) = kp.key
            && !kp.modifiers.shift() {
                return Some(Binding::Custom(send.clone()));
            }
        Binding::from_key_press(kp)
    }
}

fn chat_entries<'a>(chat: &'a Chat, md: Md<'a>) -> El<'a> {
    column(chat.entries.iter().filter_map(|e| -> Option<El<'a>> {
        Some(match e {
            ChatEntry::User(t) => container(
                column![
                    muted("You"),
                    SelectableText::<markdown::Uri, Message>::new([iced::advanced::text::Span::new(t.clone())]).size(14),
                ]
                .spacing(4),
            )
            .padding([8, 10])
            .width(Fill)
            .style(|t| style::bubble(t, true))
            .into(),
            // A reply that was nothing but the start directive.
            ChatEntry::Assistant { md: content, .. } if content.items().is_empty() => return None,
            ChatEntry::Assistant { text: raw, md: content, .. } => container(
                column![
                    row![
                        small("✦ Claude").style(|t: &Theme| text::Style { color: Some(style::purple(t)) }),
                        Space::new().width(Fill),
                        button(small(md.copy_label(raw))).padding([0, 6]).style(style::flat_button).on_press(Message::CopyText(raw.clone())),
                    ]
                    .align_y(Alignment::Center),
                    md_view(content.items(), md),
                ]
                .spacing(4),
            )
            .padding([8, 10])
            .width(Fill)
            .style(|t| style::bubble(t, false))
            .into(),
            ChatEntry::Tool(s) => muted(format!("⚙ {s}")).font(fonts::code()).into(),
            ChatEntry::Note(s) => muted(s).into(),
            ChatEntry::Error(s) => style::error_box(s, None),
        })
    }))
    .spacing(8)
    .into()
}

/// "Claude is working" with a stop button, while Claude works on the request.
fn working_line<'a>(label: &'static str, activity: &'a str) -> El<'a> {
    row![
        text(format!("✦ {label} {activity}")).size(12).width(Fill),
        button(text("■ Stop").size(12)).padding([2, 8]).style(style::default_button).on_press(Message::StopClaude),
    ]
    .align_y(Alignment::Center)
    .into()
}

fn pending_line<'a>(w: &'a Walkthrough, target: Target) -> Option<El<'a>> {
    match w.claude.pending_for(target)? {
        0 => Some(working_line("Claude is working…", &w.claude.activity)),
        n => Some(text(format!("Queued — {n} request(s) ahead")).size(12).into()),
    }
}

/// The chat panel shows what Claude is up to for everything but element chats, including
/// the silent requests.
fn panel_pending_line(w: &Walkthrough) -> Option<El<'_>> {
    let label = match w.claude.in_flight.as_ref().map(|j| j.target) {
        Some(Target::Orient) => "Getting oriented…",
        Some(Target::Plan) => "Planning the walkthrough…",
        Some(Target::Fetch(_)) => "Preparing the next element…",
        Some(Target::General) => "Claude is working…",
        Some(Target::Element(..)) | None => return pending_line(w, Target::General),
    };
    Some(working_line(label, &w.claude.activity))
}

fn chat_input<'a>(content: &'a text_editor::Content, id: String, on_action: impl Fn(text_editor::Action) -> Message + 'a, send: Message) -> iced::widget::Row<'a, Message> {
    row![
        text_editor(content)
            .id(id)
            .placeholder("Ask Claude… (Enter to send, Shift+Enter for a new line)")
            .on_action(on_action)
            .key_binding(chat_key_binding(send.clone()))
            .height(Length::Fixed(72.0))
            .size(14)
            .padding(8),
        primary_btn("Send").on_press(send),
    ]
    .spacing(8)
    .align_y(Alignment::End)
}

/// Maximum height of an element's chat log; it scrolls beyond that. The chat box has to
/// fit on screen together with the selection above it.
const ELEMENT_CHAT_LOG_MAX_HEIGHT: f32 = 320.0;

/// A walkthrough element's chat box: Claude's explanation, follow-up questions, and the
/// way to the next element.
fn element_chat_view<'a>(w: &'a Walkthrough, i: usize, e: usize, el: &'a ElementEntry, md: Md<'a>) -> El<'a> {
    let active = w.active == Some((i, e));
    let mut header = row![text(format!("✦ Lines {}", el.lines_label())).size(13).width(Fill)].align_y(Alignment::Center);
    if !active {
        header = header.push(button(text("✕").size(12)).padding([2, 8]).style(style::flat_button).on_press(Message::ToggleElement(i, e)));
    }
    // Everything above the input lives in its own column, so the input keeps the same
    // position in the widget tree (and with it, its focus) as the pending line comes and goes.
    let mut col = column![
        header,
        // Only the log is height-capped. Capping the whole column instead squeezed the
        // input out of view once the log grew past the cap, since a column hands out
        // the remaining height to its children in order.
        container(
            scrollable(container(chat_entries(&el.chat, md)).padding(Padding { right: 12.0, ..Padding::ZERO }))
                .id(element_chat_scroll_id(i, e))
                .height(Length::Shrink)
                .anchor_bottom(),
        )
        .max_height(ELEMENT_CHAT_LOG_MAX_HEIGHT),
    ]
    .spacing(8);
    if let Some(p) = pending_line(w, Target::Element(i, e)) {
        col = col.push(p);
    }
    let mut input = chat_input(&el.chat.input, element_chat_input_id(i, e), move |a| Message::ElementChatEdit(i, e, a), Message::ElementChatSend(i, e));
    if active {
        input = input.push(tooltip(
            button(text("👍").size(16)).padding([5, 12]).style(style::default_button).on_press(Message::ThumbsUp),
            container(text("Got it, next").size(12)).padding([4, 8]).style(style::menu),
            tooltip::Position::Top,
        ));
    }
    container(column![col, input].spacing(8)).id(element_chat_id(i, e)).padding(12).width(Fill).style(style::panel).into()
}

fn start_buttons(w: &Walkthrough) -> Option<El<'_>> {
    if !matches!(w.phase, Phase::Chat | Phase::Finished) {
        return None;
    }
    Some(
        row![
            primary_btn("▶ Full walkthrough").on_press(Message::StartFullWalkthrough),
            btn("▶ Part of it…").on_press(Message::StartPartialWalkthrough),
        ]
        .spacing(8)
        .into(),
    )
}

fn general_chat_view<'a>(w: &'a Walkthrough, md: Md<'a>) -> El<'a> {
    let chat = &w.general_chat;
    // The log, pending line and buttons share a column, so the input keeps the same position
    // in the widget tree (and with it, its focus) as they come and go.
    let mut log_col = column![
        scrollable(container(chat_entries(chat, md)).padding(Padding { right: 12.0, ..Padding::ZERO }))
            .id(GENERAL_CHAT_SCROLL_ID)
            .height(Fill)
            .anchor_bottom(),
    ]
    .spacing(8)
    .height(Fill);
    if let Some(p) = panel_pending_line(w) {
        log_col = log_col.push(p);
    }
    if let Some(b) = start_buttons(w) {
        log_col = log_col.push(b);
    }
    let col = column![
        row![text("✦ Claude").size(15).width(Fill), icon_btn("✕", "Hide panel", Message::ToggleChat)].align_y(Alignment::Center),
        log_col,
        chat_input(&chat.input, GENERAL_CHAT_INPUT_ID.into(), Message::GeneralChatEdit, Message::GeneralChatSend),
    ]
    .spacing(8);
    container(col).padding(12).width(400).height(Fill).style(style::panel).into()
}

// ---------------------------------------------------------------------------
// File tree
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Node {
    dirs: BTreeMap<String, Node>,
    files: Vec<usize>,
}

fn build_tree(w: &Walkthrough) -> Node {
    let mut root = Node::default();
    for (i, f) in w.files.iter().enumerate() {
        if !w.is_visible(i, f) {
            continue;
        }
        let mut node = &mut root;
        let parts: Vec<&str> = f.path.split('/').collect();
        for d in &parts[..parts.len() - 1] {
            node = node.dirs.entry((*d).to_string()).or_default();
        }
        node.files.push(i);
    }
    root
}

fn tree_rows<'a>(w: &'a Walkthrough, node: &Node, prefix: &str, depth: usize, out: &mut Vec<El<'a>>) {
    let indent = 8.0 + depth as f32 * 14.0;
    for (name, child) in &node.dirs {
        // Compress chains of single directories ("src/highlight").
        let mut label = name.clone();
        let mut full = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
        let mut child = child;
        while child.files.is_empty() && child.dirs.len() == 1 {
            let (n, c) = child.dirs.iter().next().expect("one dir");
            label = format!("{label}/{n}");
            full = format!("{full}/{n}");
            child = c;
        }
        let collapsed = w.collapsed_dirs.contains(&full);
        out.push(
            button(
                row![
                    text(if collapsed { "▸" } else { "▾" }).size(12),
                    text(label).size(13).font(fonts::ui_weight(iced::font::Weight::Semibold)),
                ]
                .spacing(6)
                .align_y(Alignment::Center),
            )
            .width(Fill)
            .padding(Padding { top: 3.0, bottom: 3.0, left: indent, right: 6.0 })
            .style(style::flat_button)
            .on_press(Message::ToggleDir(full.clone()))
            .into(),
        );
        if !collapsed {
            tree_rows(w, child, &full, depth + 1, out);
        }
    }
    for &i in &node.files {
        let f = &w.files[i];
        let name = f.path.rsplit('/').next().unwrap_or(&f.path).to_string();
        // The file the user appears to be looking at stands out.
        let weight = if w.looking_at == Some(i) { iced::font::Weight::Bold } else { iced::font::Weight::Normal };
        out.push(
            button(text(name).size(13).font(fonts::ui_weight(weight)))
                .width(Fill)
                .padding(Padding { top: 3.0, bottom: 3.0, left: indent + 18.0, right: 6.0 })
                .style(style::flat_button)
                .on_press(Message::JumpToFile(i))
                .into(),
        );
    }
}

fn file_tree_view(w: &Walkthrough) -> El<'_> {
    // Extension filter chips.
    let mut exts: BTreeMap<String, usize> = BTreeMap::new();
    for f in &w.files {
        *exts.entry(f.extension()).or_default() += 1;
    }
    let chips = row(exts.into_iter().map(|(ext, n)| {
        let on = !w.hidden_exts.contains(&ext);
        button(text(format!("{} {ext} ({n})", if on { "☑" } else { "☐" })).size(12))
            .padding([2, 6])
            .style(if on { style::default_button } else { style::flat_button })
            .on_press(Message::ToggleExt(ext))
            .into()
    }))
    .spacing(4)
    .wrap();

    let mut rows = Vec::new();
    tree_rows(w, &build_tree(w), "", 0, &mut rows);
    container(
        column![
            text_input("Filter files…", &w.filter).on_input(Message::FilterChanged).padding(6).size(13),
            chips,
            scrollable(column(rows).spacing(1)).height(Fill),
        ]
        .spacing(10),
    )
    .padding(10)
    .width(280)
    .height(Fill)
    .style(style::panel)
    .into()
}

// ---------------------------------------------------------------------------
// Status bar
// ---------------------------------------------------------------------------

fn status_bar(w: &Walkthrough) -> El<'_> {
    let (dot, label) = match &w.claude.status {
        ClaudeStatus::Starting => ("○", "starting…".to_string()),
        ClaudeStatus::Ready => (
            "●",
            if w.claude.in_flight.is_some() {
                let q = w.claude.queue.len();
                format!(
                    "busy — {}{}",
                    if w.claude.activity.is_empty() { "working…" } else { &w.claude.activity },
                    if q > 0 { format!(" ({q} queued)") } else { String::new() }
                )
            } else {
                "idle".into()
            },
        ),
        ClaudeStatus::Exited(_) => ("✕", "exited".into()),
    };
    let ready = w.claude.status == ClaudeStatus::Ready;
    let mut claude_info = format!("Claude {label}");
    if !w.claude.model.is_empty() {
        claude_info.push_str(&format!(" · {}", w.claude.model));
    }
    if !w.claude.permission_mode.is_empty() {
        claude_info.push_str(&format!(" · {}", w.claude.permission_mode));
    }
    if !w.claude.session_id.is_empty() {
        claude_info.push_str(&format!(" · session {}", &w.claude.session_id[..w.claude.session_id.len().min(8)]));
    }
    if w.claude.cost > 0.0 {
        claude_info.push_str(&format!(" · ${:.4}", w.claude.cost));
    }
    let location = match (&w.dir.branch, w.dir.is_git) {
        (Some(b), _) => format!("{} · {b}", w.dir.path.display()),
        (None, true) => format!("{} · detached HEAD", w.dir.path.display()),
        (None, false) => format!("{} · not a git repository", w.dir.path.display()),
    };
    let bar = row![
        text(dot).size(12).style(move |t: &Theme| text::Style { color: Some(if ready { style::green(t) } else { style::red(t) }) }),
        text(claude_info).size(12),
        button(text("Restart").size(11)).padding([1, 6]).style(style::flat_button).on_press(Message::RestartClaude),
        Space::new().width(Fill),
        muted(location),
    ]
    .spacing(6)
    .align_y(Alignment::Center);
    container(bar).padding([3, 10]).width(Fill).style(style::bar).into()
}

// ---------------------------------------------------------------------------
// Overlays
// ---------------------------------------------------------------------------

fn permission_view(p: &claude::PermissionRequest, pending: usize) -> El<'_> {
    let input = serde_json::to_string_pretty(&p.input).unwrap_or_default();
    let mut col = column![
        text(format!("Claude wants to use the {} tool", p.tool_name)).size(18),
    ]
    .spacing(10);
    if let Some(d) = &p.description {
        col = col.push(text(d).size(14));
    }
    if let Some(reason) = &p.decision_reason {
        col = col.push(small(format!("Reason for prompt: {reason}")).style(|t: &Theme| text::Style { color: Some(style::muted(t)) }));
    }
    col = col.push(
        container(scrollable(text(input).size(12).font(fonts::code())).height(Length::Shrink))
            .padding(10)
            .max_height(320)
            .width(Fill)
            .style(style::panel),
    );
    let mut buttons = row![Space::new().width(Fill)].spacing(8);
    if pending > 1 {
        buttons = row![small(format!("{} more request(s) waiting", pending - 1)), Space::new().width(Fill)].spacing(8);
    }
    buttons = buttons.push(
        button(text("Deny").size(13)).padding([6, 14]).style(button::danger).on_press(Message::PermissionDecision { allow: false, always: false }),
    );
    if p.suggestions.is_some() {
        buttons = buttons.push(btn("Always allow").on_press(Message::PermissionDecision { allow: true, always: true }));
    }
    buttons = buttons.push(primary_btn("Allow").on_press(Message::PermissionDecision { allow: true, always: false }));
    col = col.push(buttons.align_y(Alignment::Center));
    container(col).padding(20).width(640).style(style::modal).into()
}

fn error_modal_view(err: &str) -> El<'_> {
    container(
        column![
            text("Something went wrong").size(18),
            style::error_box(err, None),
            row![Space::new().width(Fill), primary_btn("OK").on_press(Message::DismissModal)],
        ]
        .spacing(12),
    )
    .padding(20)
    .width(640)
    .style(style::modal)
    .into()
}

fn fonts_section<'a>(ed: &'a SettingsEditor, saved_ui_font: &str) -> El<'a> {
    let label = |s: &'static str| text(s).size(13);
    let help = |s: String| text(s).size(12).style(|t: &Theme| text::Style { color: Some(style::muted(t)) });
    let missing = |name: &str, list: &[String]| -> Option<String> {
        let n = name.trim();
        (!n.is_empty() && !list.iter().any(|f| f == n)).then(|| format!("“{n}” is not installed; the default font will be used."))
    };
    let reset = |msg: Message| button(text("↺").size(13)).padding([3, 8]).style(style::flat_button).on_press(msg);

    let ui_picker = row![
        combo_box(
            &ed.ui_font_options,
            "System default",
            (!ed.ui_font.is_empty()).then_some(&ed.ui_font),
            Message::SettingsUiFont,
        )
        .on_input(Message::SettingsUiFont)
        .size(13)
        .padding(6),
        reset(Message::SettingsUiFont(String::new())),
    ]
    .spacing(4)
    .align_y(Alignment::Center);
    let code_picker = row![
        combo_box(
            &ed.code_font_options,
            "System monospace",
            (!ed.code_font.is_empty()).then_some(&ed.code_font),
            Message::SettingsCodeFont,
        )
        .on_input(Message::SettingsCodeFont)
        .size(13)
        .padding(6),
        reset(Message::SettingsCodeFont(String::new())),
    ]
    .spacing(4)
    .align_y(Alignment::Center);

    let mut ui_col = column![label("UI font"), ui_picker].spacing(4).width(Fill);
    if let Some(m) = missing(&ed.ui_font, &ed.installed_fonts.all) {
        ui_col = ui_col.push(help(m));
    } else if ed.ui_font.trim() != saved_ui_font {
        ui_col = ui_col.push(help("Applied after a restart (Save relaunches the app on the current directory).".into()));
    }
    let mut code_col = column![label("Code font"), code_picker].spacing(4).width(Fill);
    if let Some(m) = missing(&ed.code_font, &ed.installed_fonts.all) {
        code_col = code_col.push(help(m));
    }

    let sliders = row![
        column![
            label("Code font size"),
            row![
                slider(9.0..=22.0, ed.code_font_size, Message::SettingsCodeSize).step(0.5_f32),
                text(format!("{:.1} px", ed.code_font_size)).size(12).width(56),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        ]
        .spacing(4)
        .width(Fill),
        column![
            label("UI zoom"),
            row![
                slider(0.75..=2.0, ed.ui_scale, Message::SettingsUiScale).step(0.05_f32),
                text(format!("{:.0} %", ed.ui_scale * 100.0)).size(12).width(56),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        ]
        .spacing(4)
        .width(Fill),
    ]
    .spacing(16);

    let preview_ui = fonts::family(&ed.ui_font, Font::DEFAULT);
    let preview = container(
        column![
            row![
                text("src/greeter.rs").size(16).font(Font { weight: iced::font::Weight::Semibold, ..preview_ui }),
                text("4 lines").size(16).font(preview_ui).style(|t: &Theme| text::Style { color: Some(style::muted(t)) }),
            ]
            .spacing(6),
            text("The quick brown fox jumps over the lazy dog.").size(13).font(preview_ui),
            container(
                CodeView::new(&ed.sample.lines, &ed.sample.spans, Colors::new(ed.dark_theme))
                    .font(fonts::family(&ed.code_font, Font::MONOSPACE), ed.code_font_size),
            )
            .style(style::card),
        ]
        .spacing(6),
    )
    .padding(12)
    .width(Fill)
    .style(style::panel);

    column![
        text("Fonts").size(15),
        help("Pick any installed family (type to search; monospace fonts are listed first for code). Code font and sizes apply on Save; UI zoom scales the whole window.".into()),
        row![ui_col, code_col].spacing(16),
        sliders,
        small("Preview"),
        preview,
    ]
    .spacing(8)
    .into()
}

fn settings_view<'a>(ed: &'a SettingsEditor, saved_ui_font: &'a str) -> El<'a> {
    fn editor<'a>(_c: &'static str, content: &'a text_editor::Content, field: SettingsField, h: f32) -> El<'a> {
        text_editor(content)
            .on_action(move |a| Message::SettingsEdit(field, a))
            .height(Length::Fixed(h))
            .size(13)
            .font(fonts::code())
            .padding(8)
            .into()
    }
    let help = |s: &'static str| text(s).size(12).style(|t: &Theme| text::Style { color: Some(style::muted(t)) });
    let mut col = column![
        row![text("Settings").size(20), Space::new().width(Fill), btn("Close").on_press(Message::CloseSettings)].align_y(Alignment::Center),
        row![text("Dark theme").size(15).width(Fill), toggler(ed.dark_theme).on_toggle(Message::SettingsDarkTheme)].align_y(Alignment::Center),
        row![
            column![
                text("Wrap long lines").size(15),
                help("Default for files; the ↩ Wrap button on each file overrides it for that file."),
            ]
            .width(Fill),
            toggler(ed.wrap_lines).on_toggle(Message::SettingsWrapLines),
        ]
        .align_y(Alignment::Center),
        row![
            column![
                text("Prepare the next element ahead of time").size(15),
                help("Claude works on the next element while you look at the current one, so 👍 usually moves on at once. Asking a question discards that work, which costs some extra tokens."),
            ]
            .width(Fill),
            toggler(ed.prefetch).on_toggle(Message::SettingsPrefetch),
        ]
        .align_y(Alignment::Center),
        fonts_section(ed, saved_ui_font),
        text("Claude command").size(15),
        help("Shell command (run with `sh -c` in the directory) that starts Claude Code in stream-json mode. `{dir}` is replaced with the shell-quoted directory path. For Docker, use `docker exec -i` (not `-it`: stdin is a pipe, not a TTY), e.g. docker exec -i -w {dir} dockerclaude claude -p --input-format stream-json …"),
        editor("cmd", &ed.command, SettingsField::Command, 110.0),
        text("Meta prompt: orientation").size(15),
        help("Sent silently when the walkthrough screen opens. Placeholders: {dir} {git_info}."),
        editor("orient", &ed.orient_prompt, SettingsField::OrientPrompt, 170.0),
        text("Meta prompt: general chat").size(15),
        help("Wrapped around messages typed into the chat panel. Not shown in the chat log. Placeholders: {message} {dir} {git_info} {status}. If {message} is missing, the message is appended. To start a walkthrough from the chat, Claude's reply must contain [[START_WALKTHROUGH: <scope>]]."),
        editor("general", &ed.general_chat_meta, SettingsField::GeneralChatMeta, 170.0),
        text("Meta prompt: file order").size(15),
        help("Sent silently when a walkthrough starts. Placeholders: {scope} {dir} {git_info}. The response must contain a JSON object {\"files\": [{\"path\": \"…\", \"why\": \"…\"}]} with paths relative to the directory."),
        editor("plan", &ed.plan_prompt, SettingsField::PlanPrompt, 170.0),
        text("Meta prompt: next element").size(15),
        help("Sent silently for each walkthrough element. Placeholders: {position} {path} {why} {covered} {dir} {git_info}. The response must contain a JSON object {\"start_line\": 1, \"end_line\": 5, \"comment\": \"…\"} or {\"done\": true}."),
        editor("element", &ed.element_prompt, SettingsField::ElementPrompt, 220.0),
        text("Meta prompt: element chat").size(15),
        help("Wrapped around messages typed into an element's chat box. Not shown in the chat log. Placeholders: {message} {path} {lines} {dir} {git_info} {status}."),
        editor("element-chat", &ed.element_chat_meta, SettingsField::ElementChatMeta, 110.0),
        small(format!("Saved to {}", crate::settings::Settings::path().display())),
    ]
    .spacing(8);
    if let Some(e) = &ed.error {
        col = col.push(style::error_box(e, None));
    }
    col = col.push(
        row![
            btn("Reset to defaults").on_press(Message::ResetSettings),
            Space::new().width(Fill),
            btn("Cancel").on_press(Message::CloseSettings),
            primary_btn(if ed.ui_font.trim() != saved_ui_font { "Save & restart" } else { "Save" }).on_press(Message::SaveSettings),
        ]
        .spacing(8),
    );
    container(scrollable(container(col).padding(Padding { right: 14.0, ..Padding::ZERO })))
        .padding(20)
        .width(820)
        .max_height(860)
        .style(style::modal)
        .into()
}
