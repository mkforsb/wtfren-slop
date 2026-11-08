//! Application state, messages and update logic.

use crate::claude::{self, Outgoing, PermissionRequest};
use crate::code_view::{self, Line};
use crate::files::{self, DirInfo, LoadedFile};
use crate::highlight::{self, LineSpans};
use crate::settings::{self, PromptVars, Settings};
use crate::walkthrough::{self, Element, Fetch, Step, Walk};
use iced::widget::{markdown, operation, text_editor};
use iced::{Subscription, Task};
use std::collections::{BTreeSet, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc::UnboundedSender;

pub const PAGE_SCROLL_ID: &str = "page-scroll";
pub const GENERAL_CHAT_SCROLL_ID: &str = "general-chat-scroll";
pub const GENERAL_CHAT_INPUT_ID: &str = "general-chat-input";

pub fn file_section_id(i: usize) -> String {
    format!("file-section-{i}")
}

pub fn element_chat_id(i: usize, e: usize) -> String {
    format!("element-chat-{i}-{e}")
}

pub fn element_chat_scroll_id(i: usize, e: usize) -> String {
    format!("element-chat-scroll-{i}-{e}")
}

pub fn element_chat_input_id(i: usize, e: usize) -> String {
    format!("element-chat-input-{i}-{e}")
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

pub struct App {
    pub settings: Settings,
    pub screen: Screen,
    pub settings_editor: Option<SettingsEditor>,
    pub window_size: iced::Size,
    pub modal_error: Option<String>,
    /// The most recently copied text, while its button shows "Copied!", with the
    /// copy's sequence number (so an older timer can't clear a newer copy).
    pub copied: Option<(u64, String)>,
    copy_seq: u64,
}

/// How long a copy button says "Copied!".
const COPIED_FEEDBACK: std::time::Duration = std::time::Duration::from_secs(1);

pub enum Screen {
    DirSelect(DirSelect),
    Walkthrough(Box<Walkthrough>),
}

#[derive(Default)]
pub struct DirSelect {
    pub path_input: String,
    pub errors: Vec<String>,
    pub busy: bool,
}

pub struct SettingsEditor {
    pub command: text_editor::Content,
    pub orient_prompt: text_editor::Content,
    pub general_chat_meta: text_editor::Content,
    pub plan_prompt: text_editor::Content,
    pub element_prompt: text_editor::Content,
    pub element_chat_meta: text_editor::Content,
    pub prefetch: bool,
    pub dark_theme: bool,
    pub wrap_lines: bool,
    pub error: Option<String>,
    pub ui_font: String,
    pub code_font: String,
    pub code_font_size: f32,
    pub ui_scale: f32,
    pub installed_fonts: crate::fonts::Installed,
    pub ui_font_options: iced::widget::combo_box::State<String>,
    pub code_font_options: iced::widget::combo_box::State<String>,
    /// A small highlighted file used to preview the code font.
    pub sample: CodeModel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsField {
    Command,
    OrientPrompt,
    GeneralChatMeta,
    PlanPrompt,
    ElementPrompt,
    ElementChatMeta,
}

impl SettingsEditor {
    fn new(s: &Settings) -> Self {
        Self {
            command: text_editor::Content::with_text(&s.claude_command),
            orient_prompt: text_editor::Content::with_text(&s.orient_prompt),
            general_chat_meta: text_editor::Content::with_text(&s.general_chat_meta),
            plan_prompt: text_editor::Content::with_text(&s.plan_prompt),
            element_prompt: text_editor::Content::with_text(&s.element_prompt),
            element_chat_meta: text_editor::Content::with_text(&s.element_chat_meta),
            prefetch: s.prefetch,
            dark_theme: s.dark_theme,
            wrap_lines: s.wrap_lines,
            error: None,
            ui_font: s.ui_font.clone(),
            code_font: s.code_font.clone(),
            code_font_size: s.code_font_size,
            ui_scale: s.ui_scale,
            ui_font_options: iced::widget::combo_box::State::new(Vec::new()),
            code_font_options: iced::widget::combo_box::State::new(Vec::new()),
            installed_fonts: Default::default(),
            sample: sample_model(s.dark_theme),
        }
    }

    /// Fills the font pickers with the installed families (monospace ones first for code).
    fn load_fonts(&mut self) {
        let installed = crate::fonts::installed();
        let mut code: Vec<String> = installed.monospace.clone();
        code.extend(installed.all.iter().filter(|f| !installed.monospace.contains(f)).cloned());
        self.ui_font_options = iced::widget::combo_box::State::new(installed.all.clone());
        self.code_font_options = iced::widget::combo_box::State::new(code);
        self.installed_fonts = installed;
    }

    pub fn field_mut(&mut self, f: SettingsField) -> &mut text_editor::Content {
        match f {
            SettingsField::Command => &mut self.command,
            SettingsField::OrientPrompt => &mut self.orient_prompt,
            SettingsField::GeneralChatMeta => &mut self.general_chat_meta,
            SettingsField::PlanPrompt => &mut self.plan_prompt,
            SettingsField::ElementPrompt => &mut self.element_prompt,
            SettingsField::ElementChatMeta => &mut self.element_chat_meta,
        }
    }
}

const SAMPLE: &str = "/// Greets someone politely.\nfn greet(name: &str) -> String {\n    format!(\"Hello, {name}! 0O 1lI {{}} => != ->\")\n}\n";

fn sample_model(dark: bool) -> CodeModel {
    CodeModel { lines: code_view::lines_of(SAMPLE), spans: highlight::highlight_file("sample.rs", SAMPLE, dark) }
}

/// Text with a trailing newline stripped (text_editor always has one).
fn editor_text(c: &text_editor::Content) -> String {
    let t = c.text();
    t.strip_suffix('\n').map(str::to_owned).unwrap_or(t)
}

#[derive(Debug, Clone, Default)]
pub struct CodeModel {
    pub lines: Vec<Line>,
    pub spans: Vec<LineSpans>,
}

// Chats hold few entries, so keeping parsed markdown inline isn't worth a box.
#[allow(clippy::large_enum_variant)]
pub enum ChatEntry {
    User(String),
    Assistant { msg_id: String, text: String, md: markdown::Content },
    Tool(String),
    /// Something the tool or Claude says in passing, such as Claude's remarks between
    /// tool calls while it works on a request the user didn't type.
    Note(String),
    Error(String),
}

pub struct Chat {
    pub entries: Vec<ChatEntry>,
    pub input: text_editor::Content,
}

impl Default for Chat {
    fn default() -> Self {
        Self { entries: Vec::new(), input: text_editor::Content::new() }
    }
}

/// A walkthrough element on the page: a selection, and its chat under it.
pub struct ElementEntry {
    /// First and last selected line, 1-based.
    pub start: u32,
    pub end: u32,
    pub chat: Chat,
    pub expanded: bool,
}

impl ElementEntry {
    fn new(start: u32, end: u32, comment: String) -> Self {
        let md = markdown::Content::parse(&comment);
        let entries = vec![ChatEntry::Assistant { msg_id: String::new(), text: comment, md }];
        Self { start, end, chat: Chat { entries, ..Chat::default() }, expanded: true }
    }

    /// "12", or "12–20".
    pub fn lines_label(&self) -> String {
        if self.start == self.end { self.start.to_string() } else { format!("{}–{}", self.start, self.end) }
    }
}

pub struct FileEntry {
    pub path: String,
    pub why: String,
    pub text: Arc<String>,
    pub code: CodeModel,
    /// Per-file override of [`Settings::wrap_lines`].
    pub wrap: Option<bool>,
    pub collapsed: bool,
    pub elements: Vec<ElementEntry>,
}

impl FileEntry {
    pub fn wraps(&self, default: bool) -> bool {
        self.wrap.unwrap_or(default)
    }

    pub fn extension(&self) -> String {
        ext_of(&self.path)
    }
}

pub fn ext_of(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => format!(".{ext}"),
        _ => "No extension".into(),
    }
}

/// What a request to Claude is for, which decides where its output goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// The silent first prompt; the reply goes to the chat panel.
    Orient,
    /// The chat panel.
    General,
    /// The file order of the walkthrough.
    Plan,
    /// The next walkthrough element.
    Fetch(Fetch),
    /// The chat box of an element.
    Element(usize, usize),
}

impl Target {
    /// Whether Claude's text is meant for the user, rather than for the tool to parse.
    fn is_chat(self) -> bool {
        matches!(self, Target::Orient | Target::General | Target::Element(..))
    }

    /// Whether the output belongs in the chat panel.
    pub fn in_panel(self) -> bool {
        !matches!(self, Target::Element(..))
    }
}

pub struct ClaudeJob {
    pub target: Target,
    pub prompt: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ClaudeStatus {
    Starting,
    Ready,
    Exited(String),
}

pub struct ClaudeState {
    pub generation: u64,
    pub sender: Option<UnboundedSender<Outgoing>>,
    pub status: ClaudeStatus,
    pub queue: VecDeque<ClaudeJob>,
    pub in_flight: Option<ClaudeJob>,
    pub model: String,
    pub session_id: String,
    pub permission_mode: String,
    pub cost: f64,
    pub current_msg: String,
    pub activity: String,
    pub interrupting: bool,
    pub permissions: VecDeque<PermissionRequest>,
}

impl ClaudeState {
    fn new(generation: u64) -> Self {
        Self {
            generation,
            sender: None,
            status: ClaudeStatus::Starting,
            queue: VecDeque::new(),
            in_flight: None,
            model: String::new(),
            session_id: String::new(),
            permission_mode: String::new(),
            cost: 0.0,
            current_msg: String::new(),
            activity: String::new(),
            interrupting: false,
            permissions: VecDeque::new(),
        }
    }

    /// Whether a job for `target` is running or queued.
    pub fn pending_for(&self, target: Target) -> Option<usize> {
        if self.in_flight.as_ref().is_some_and(|j| j.target == target) {
            return Some(0);
        }
        self.queue.iter().position(|j| j.target == target).map(|p| p + 1)
    }
}

/// Where the walkthrough stands.
pub enum Phase {
    /// Claude is getting oriented.
    Orienting,
    /// Chatting about the codebase in general.
    Chat,
    /// Claude is deciding on the file order.
    Planning,
    Walking(Walk),
    /// All files are done; chatting goes on, and another walkthrough can start.
    Finished,
}

pub struct Walkthrough {
    pub dir: DirInfo,
    pub files: Vec<FileEntry>,
    pub errors: Vec<String>,
    pub filter: String,
    pub hidden_exts: BTreeSet<String>,
    pub collapsed_dirs: HashSet<String>,
    pub general_chat: Chat,
    pub show_chat: bool,
    pub claude: ClaudeState,
    pub phase: Phase,
    /// Claude finished orienting in this session.
    pub oriented: bool,
    /// The file the walkthrough is at, as an index into `files`.
    pub file: Option<usize>,
    /// The number of `file`'s elements from an earlier walkthrough, which don't count
    /// as covered in this one.
    pub earlier_elements: usize,
    /// The element the user is looking at: (file, element).
    pub active: Option<(usize, usize)>,
    /// The file the user appears to be looking at.
    pub looking_at: Option<usize>,
}

impl Walkthrough {
    fn new(dir: DirInfo, generation: u64) -> Self {
        Self {
            dir,
            files: Vec::new(),
            errors: Vec::new(),
            filter: String::new(),
            hidden_exts: BTreeSet::new(),
            collapsed_dirs: HashSet::new(),
            general_chat: Chat::default(),
            show_chat: true,
            claude: ClaudeState::new(generation),
            phase: Phase::Orienting,
            oriented: false,
            file: None,
            earlier_elements: 0,
            active: None,
            looking_at: None,
        }
    }

    pub fn walk(&self) -> Option<&Walk> {
        match &self.phase {
            Phase::Walking(w) => Some(w),
            _ => None,
        }
    }

    fn walk_mut(&mut self) -> Option<&mut Walk> {
        match &mut self.phase {
            Phase::Walking(w) => Some(w),
            _ => None,
        }
    }

    /// The file the walkthrough is at always stays on the page.
    pub fn is_visible(&self, i: usize, f: &FileEntry) -> bool {
        if self.file == Some(i) {
            return true;
        }
        if self.hidden_exts.contains(&f.extension()) {
            return false;
        }
        let q = self.filter.trim().to_lowercase();
        q.is_empty() || f.path.to_lowercase().contains(&q)
    }

    fn base_vars(&self) -> PromptVars {
        let dir = self.dir.path.display().to_string();
        let git_info = match (self.dir.is_git, &self.dir.branch) {
            (true, Some(b)) => format!("It is a git repository, on branch {b}, so the history can tell you a lot as well."),
            (true, None) => "It is a git repository, so the history can tell you a lot as well.".to_string(),
            (false, _) => "It is not a git repository.".to_string(),
        };
        let status = match &self.phase {
            Phase::Walking(w) => match w.plan.get(w.current) {
                Some(item) => format!("in progress, at file {} of {} ({})", w.current + 1, w.plan.len(), item.path),
                None => "in progress".to_string(),
            },
            Phase::Planning => "being planned".to_string(),
            Phase::Finished => "finished".to_string(),
            Phase::Orienting | Phase::Chat => "not started".to_string(),
        };
        PromptVars { dir, git_info, status, ..Default::default() }
    }

    fn chat_mut(&mut self, target: Target) -> Option<&mut Chat> {
        match target {
            Target::Element(i, e) => self.files.get_mut(i)?.elements.get_mut(e).map(|el| &mut el.chat),
            _ => Some(&mut self.general_chat),
        }
    }

    fn enqueue(&mut self, job: ClaudeJob) {
        self.claude.queue.push_back(job);
        self.pump();
    }

    /// Sends the next queued job if Claude is idle.
    fn pump(&mut self) {
        if self.claude.in_flight.is_some() {
            return;
        }
        let Some(tx) = &self.claude.sender else { return };
        let Some(job) = self.claude.queue.pop_front() else { return };
        if tx.send(Outgoing::User(job.prompt.clone())).is_err() {
            self.claude.queue.push_front(job);
            return;
        }
        self.claude.current_msg.clear();
        self.claude.activity = "thinking…".into();
        self.claude.in_flight = Some(job);
    }

    fn enqueue_orient(&mut self, settings: &Settings) {
        self.phase = Phase::Orienting;
        let prompt = settings::render_template(&settings.orient_prompt, &self.base_vars(), "");
        self.enqueue(ClaudeJob { target: Target::Orient, prompt });
    }

    fn fetch_prompt(&self, fetch: Fetch, settings: &Settings) -> Option<String> {
        let walk = self.walk()?;
        let item = walk.plan.get(fetch.plan_idx)?;
        // A fetch ahead of the current file is for a file nothing was shown of yet.
        let shown = self.file.filter(|_| fetch.plan_idx == walk.current).and_then(|i| self.files.get(i));
        let ranges = shown.iter().flat_map(|f| f.elements.iter().skip(self.earlier_elements)).map(|e| (e.start, e.end));
        let vars = PromptVars {
            path: item.path.clone(),
            why: item.why.clone(),
            position: format!("{} of {}", fetch.plan_idx + 1, walk.plan.len()),
            covered: walkthrough::covered_text(ranges),
            ..self.base_vars()
        };
        Some(settings::render_template(&settings.element_prompt, &vars, ""))
    }

    fn enqueue_fetch(&mut self, fetch: Fetch, settings: &Settings) {
        if let Some(prompt) = self.fetch_prompt(fetch, settings) {
            self.enqueue(ClaudeJob { target: Target::Fetch(fetch), prompt });
        }
    }

    /// When Claude has nothing else to do, gets the next element ready.
    fn maybe_prefetch(&mut self, settings: &Settings) {
        if !settings.prefetch || self.claude.in_flight.is_some() || !self.claude.queue.is_empty() {
            return;
        }
        if let Some(fetch) = self.walk_mut().and_then(Walk::prefetch) {
            self.enqueue_fetch(fetch, settings);
        }
    }

    /// The user is about to talk to Claude. A prefetch in the way would delay the answer,
    /// and may be out of date afterwards, so it's dropped; see [`Walk::invalidate_prefetch`].
    fn cancel_prefetch(&mut self) {
        let Some(fetch) = self.walk_mut().and_then(Walk::invalidate_prefetch) else { return };
        let target = Target::Fetch(fetch);
        self.claude.queue.retain(|j| j.target != target);
        if self.claude.in_flight.as_ref().is_some_and(|j| j.target == target)
            && let Some(tx) = &self.claude.sender
        {
            let _ = tx.send(Outgoing::Interrupt);
            self.claude.interrupting = true;
        }
    }

    fn fail_all_jobs(&mut self, reason: &str) {
        let jobs: Vec<ClaudeJob> = self.claude.in_flight.take().into_iter().chain(self.claude.queue.drain(..)).collect();
        for job in jobs {
            self.finish_job_with_error(job.target, reason);
        }
    }

    fn finish_job_with_error(&mut self, target: Target, reason: &str) {
        match target {
            Target::Fetch(f) => {
                if let Some(w) = self.walk_mut() {
                    w.on_failure(f.ticket, reason);
                }
            }
            t => {
                if matches!(t, Target::Orient | Target::Plan) {
                    self.phase = Phase::Chat;
                }
                if let Some(c) = self.chat_mut(t) {
                    c.entries.push(ChatEntry::Error(reason.to_string()));
                }
            }
        }
    }

    /// Starts a new Claude session. Prompts carry what Claude needs to carry on with the
    /// walkthrough, but without an orientation there's nothing to chat about yet.
    fn restart_claude(&mut self, reason: &str, settings: &Settings) {
        self.fail_all_jobs(reason);
        self.claude = ClaudeState::new(self.claude.generation + 1);
        if !self.oriented && matches!(self.phase, Phase::Chat) {
            self.enqueue_orient(settings);
        }
    }

    /// How far the walkthrough has come, for the top bar.
    pub fn progress(&self) -> String {
        match &self.phase {
            Phase::Orienting => "Claude is getting oriented…".into(),
            Phase::Chat => "General chat".into(),
            Phase::Planning => "Claude is planning the file order…".into(),
            Phase::Walking(w) => {
                let n = self.active.map(|(i, e)| format!(" · element {}", e + 1 - self.earlier_in(i))).unwrap_or_default();
                format!("File {} of {}{n}", (w.current + 1).min(w.plan.len()), w.plan.len())
            }
            Phase::Finished => "Walkthrough finished".into(),
        }
    }

    fn earlier_in(&self, i: usize) -> usize {
        if self.file == Some(i) { self.earlier_elements } else { 0 }
    }
}

// ---------------------------------------------------------------------------
// Messages
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum Message {
    Noop,
    WindowEvent(iced::Event),
    CopyText(String),
    CopiedExpired(u64),
    // Settings
    OpenSettings,
    CloseSettings,
    SaveSettings,
    ResetSettings,
    SettingsEdit(SettingsField, text_editor::Action),
    SettingsPrefetch(bool),
    SettingsDarkTheme(bool),
    SettingsWrapLines(bool),
    SettingsUiFont(String),
    SettingsCodeFont(String),
    SettingsCodeSize(f32),
    SettingsUiScale(f32),
    DismissModal,
    // Directory select
    PickDir,
    DirPicked(Option<PathBuf>, bool),
    DirPathChanged(String),
    OpenDirPath,
    OpenRecent(PathBuf),
    DirVerified(Result<DirInfo, String>),
    DismissError(usize),
    // Walkthrough screen
    BackToDirSelect,
    ToggleTree,
    ToggleChat,
    ToggleCollapsed(usize),
    ToggleWrap(usize),
    ExpandAll,
    CollapseAll,
    FilterChanged(String),
    ToggleExt(String),
    ToggleDir(String),
    JumpToFile(usize),
    LookingAt(Option<usize>),
    ScrollTo(Option<f32>),
    LinkClicked(markdown::Uri),
    StartFullWalkthrough,
    StartPartialWalkthrough,
    FileLoaded(usize, Result<LoadedFile, String>),
    ToggleElement(usize, usize),
    ThumbsUp,
    RetryElement,
    ElementChatEdit(usize, usize, text_editor::Action),
    ElementChatSend(usize, usize),
    GeneralChatEdit(text_editor::Action),
    GeneralChatSend,
    StopClaude,
    RestartClaude,
    Claude(claude::Event),
    PermissionDecision { allow: bool, always: bool },
    Rehighlighted(Vec<(usize, Vec<LineSpans>)>),
}

// ---------------------------------------------------------------------------
// Scrolling to things on the page
// ---------------------------------------------------------------------------

/// Where a container sits inside a scrollable, in the scrollable's content coordinates.
#[derive(Debug, Clone, Copy)]
pub struct Located {
    /// Top of the target, relative to the top of the scrollable content.
    pub y: f32,
    pub height: f32,
    /// Height of the scrollable's visible area.
    pub viewport: f32,
    /// Current vertical scroll offset.
    pub scroll: f32,
    /// Top of the enclosing section, if asked for.
    pub section_y: Option<f32>,
}

impl Located {
    /// Offset that puts the top of the target just below the top of the viewport.
    fn top(self) -> f32 {
        (self.y - 8.0).max(0.0)
    }

    /// Offset that brings the bottom of the target into view, or `None` if the
    /// whole target is already visible.
    fn reveal_bottom(self) -> Option<f32> {
        let bottom = self.y + self.height + 8.0;
        let fully_visible = self.y >= self.scroll && bottom <= self.scroll + self.viewport;
        (!fully_visible).then(|| (bottom - self.viewport).max(0.0))
    }

    /// Offset that puts the bottom of the target at the very bottom of the viewport, which
    /// shows as much as possible of what's above it. Near the top of a section that would
    /// bring the previous file into view for no reason, so it stops at the section's top.
    fn pin_to_bottom(self) -> f32 {
        let pinned = (self.y + self.height + 8.0 - self.viewport).max(0.0);
        self.section_y.map_or(pinned, |y| pinned.max(y - 8.0))
    }
}

/// Finds a container inside a scrollable.
struct Locate {
    scrollable: iced::widget::Id,
    target: iced::widget::Id,
    viewport: Option<(f32, f32, f32)>,
    target_bounds: Option<iced::Rectangle>,
    section: Option<iced::widget::Id>,
    section_bounds: Option<iced::Rectangle>,
    /// If set, only report the target while this focusable widget has focus.
    require_focus: Option<iced::widget::Id>,
    has_focus: bool,
}

impl Locate {
    fn new(scrollable: impl Into<iced::widget::Id>, target: impl Into<iced::widget::Id>) -> Self {
        Self {
            scrollable: scrollable.into(),
            target: target.into(),
            viewport: None,
            target_bounds: None,
            section: None,
            section_bounds: None,
            require_focus: None,
            has_focus: false,
        }
    }

    fn in_section(mut self, id: impl Into<iced::widget::Id>) -> Self {
        self.section = Some(id.into());
        self
    }

    fn if_focused(mut self, id: impl Into<iced::widget::Id>) -> Self {
        self.require_focus = Some(id.into());
        self
    }
}

impl iced::advanced::widget::Operation<Option<Located>> for Locate {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn iced::advanced::widget::Operation<Option<Located>>)) {
        operate(self);
    }

    fn container(&mut self, id: Option<&iced::widget::Id>, bounds: iced::Rectangle) {
        if id == Some(&self.target) {
            self.target_bounds = Some(bounds);
        }
        if id.is_some() && id == self.section.as_ref() {
            self.section_bounds = Some(bounds);
        }
    }

    fn focusable(
        &mut self,
        id: Option<&iced::widget::Id>,
        _bounds: iced::Rectangle,
        state: &mut dyn iced::advanced::widget::operation::Focusable,
    ) {
        if id.is_some() && id == self.require_focus.as_ref() {
            self.has_focus = state.is_focused();
        }
    }

    fn scrollable(
        &mut self,
        id: Option<&iced::widget::Id>,
        bounds: iced::Rectangle,
        content_bounds: iced::Rectangle,
        translation: iced::Vector,
        _state: &mut dyn iced::advanced::widget::operation::Scrollable,
    ) {
        if id == Some(&self.scrollable) {
            self.viewport = Some((content_bounds.y, bounds.height, translation.y));
        }
    }

    fn finish(&self) -> iced::advanced::widget::operation::Outcome<Option<Located>> {
        if self.require_focus.is_some() && !self.has_focus {
            return iced::advanced::widget::operation::Outcome::Some(None);
        }
        iced::advanced::widget::operation::Outcome::Some(match (self.viewport, self.target_bounds) {
            (Some((content_y, viewport, scroll)), Some(t)) => Some(Located {
                y: t.y - content_y,
                height: t.height,
                viewport,
                scroll,
                section_y: self.section_bounds.map(|s| s.y - content_y),
            }),
            _ => None,
        })
    }
}

fn scroll_to_file(i: usize) -> Task<Message> {
    iced::advanced::widget::operate(Locate::new(PAGE_SCROLL_ID, file_section_id(i))).map(|l| Message::ScrollTo(l.map(Located::top)))
}

/// Puts a new element's chat box at the bottom of the page view, with the selection above
/// it, and focuses the input.
fn present_element(i: usize, e: usize) -> Task<Message> {
    let locate = Locate::new(PAGE_SCROLL_ID, element_chat_id(i, e)).in_section(file_section_id(i));
    Task::batch([
        iced::advanced::widget::operate(locate).map(|l| Message::ScrollTo(l.map(Located::pin_to_bottom))),
        // A long explanation should start at its beginning. See `scroll_chat_to_end` for
        // why that's the end of a log anchored to the bottom.
        operation::snap_to(element_chat_scroll_id(i, e), operation::RelativeOffset::END),
        operation::focus(element_chat_input_id(i, e)),
    ])
}

/// Scrolls the page so the whole chat box of an element, including its input at the
/// bottom, is visible. With `only_if_focused`, does so only while the input has keyboard
/// focus, so a user who has moved on to the code isn't pulled back.
fn reveal_element_chat(i: usize, e: usize, only_if_focused: bool) -> Task<Message> {
    let mut locate = Locate::new(PAGE_SCROLL_ID, element_chat_id(i, e));
    if only_if_focused {
        locate = locate.if_focused(element_chat_input_id(i, e));
    }
    iced::advanced::widget::operate(locate).map(|l| Message::ScrollTo(l.and_then(Located::reveal_bottom)))
}

fn scroll_chat_to_end(target: Target) -> Task<Message> {
    // The chat logs use `anchor_bottom()`, which measures the offset from the bottom:
    // relative offset 0 is the newest entry (and keeps following it as the log grows),
    // while `snap_to_end` (offset 1.0) would jump to the oldest one.
    match target {
        // The chat box grows with the log until the log hits its height cap, which can
        // push the input below the window edge.
        Target::Element(i, e) => Task::batch([
            operation::snap_to(element_chat_scroll_id(i, e), operation::RelativeOffset::START),
            reveal_element_chat(i, e, true),
        ]),
        _ => operation::snap_to(GENERAL_CHAT_SCROLL_ID, operation::RelativeOffset::START),
    }
}

// ---------------------------------------------------------------------------
// App
// ---------------------------------------------------------------------------

impl App {
    pub fn new() -> (Self, Task<Message>) {
        let settings = Settings::load();
        // Allow `wtfren <dir>` to jump straight to the walkthrough.
        let arg = std::env::args().nth(1).map(PathBuf::from);
        let app = App {
            settings,
            screen: Screen::DirSelect(DirSelect::default()),
            settings_editor: None,
            window_size: iced::Size::new(1500.0, 950.0),
            modal_error: None,
            copied: None,
            copy_seq: 0,
        };
        let task = match arg {
            Some(p) => Task::perform(files::verify_dir(p), Message::DirVerified),
            None => Task::none(),
        };
        (app, task)
    }

    pub fn title(&self) -> String {
        match &self.screen {
            Screen::Walkthrough(w) => format!("{} — Codebase Walkthrough", w.dir.path.display()),
            _ => "Codebase Walkthrough".into(),
        }
    }

    /// Text whose copy button currently says "Copied!".
    pub fn copied(&self) -> Option<&str> {
        self.copied.as_ref().map(|(_, t)| t.as_str())
    }

    pub fn scale_factor(&self) -> f32 {
        self.settings.ui_scale.clamp(0.5, 3.0)
    }

    fn relaunch(&self) -> Result<(), String> {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let dir = match &self.screen {
            Screen::Walkthrough(w) => Some(w.dir.path.clone()),
            Screen::DirSelect(_) => None,
        };
        let mut cmd = std::process::Command::new(exe);
        cmd.args(dir);
        cmd.spawn().map(|_| ()).map_err(|e| e.to_string())
    }

    pub fn theme(&self) -> iced::Theme {
        crate::style::theme(self.settings.dark_theme)
    }

    pub fn subscription(&self) -> Subscription<Message> {
        let mut subs = vec![iced::event::listen_with(|event, _status, _window| match &event {
            iced::Event::Window(iced::window::Event::Resized(_))
            | iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
                key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape),
                ..
            }) => Some(Message::WindowEvent(event)),
            _ => None,
        })];
        if let Screen::Walkthrough(w) = &self.screen {
            let cfg = claude::Config {
                command: self.settings.claude_command.clone(),
                dir: w.dir.path.clone(),
                generation: w.claude.generation,
            };
            subs.push(Subscription::run_with(cfg, claude::connect).map(Message::Claude));
        }
        Subscription::batch(subs)
    }

    fn open_dir(&mut self, dir: DirInfo) -> Task<Message> {
        self.settings.add_recent_dir(dir.path.clone());
        let _ = self.settings.save();
        let generation = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let mut w = Walkthrough::new(dir, generation);
        w.enqueue_orient(&self.settings);
        self.screen = Screen::Walkthrough(Box::new(w));
        operation::focus(GENERAL_CHAT_INPUT_ID)
    }

    fn verify(&mut self, path: PathBuf) -> Task<Message> {
        if let Screen::DirSelect(s) = &mut self.screen {
            s.path_input = path.display().to_string();
            s.busy = true;
        }
        Task::perform(files::verify_dir(path), Message::DirVerified)
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Noop => Task::none(),
            Message::WindowEvent(event) => {
                match event {
                    iced::Event::Window(iced::window::Event::Resized(size)) => self.window_size = size,
                    // Escape closes the topmost transient UI.
                    iced::Event::Keyboard(_) if self.modal_error.take().is_none() => self.settings_editor = None,
                    _ => {}
                }
                Task::none()
            }
            Message::DismissModal => {
                self.modal_error = None;
                Task::none()
            }

            // ----------------------------------------------------------- settings
            Message::OpenSettings => {
                let mut ed = SettingsEditor::new(&self.settings);
                ed.load_fonts();
                self.settings_editor = Some(ed);
                Task::none()
            }
            Message::CloseSettings => {
                self.settings_editor = None;
                Task::none()
            }
            Message::ResetSettings => {
                let defaults = Settings { dark_theme: self.settings.dark_theme, ..Settings::default() };
                let mut ed = SettingsEditor::new(&defaults);
                ed.load_fonts();
                self.settings_editor = Some(ed);
                Task::none()
            }
            Message::SettingsEdit(field, action) => {
                if let Some(ed) = &mut self.settings_editor {
                    ed.field_mut(field).perform(action);
                }
                Task::none()
            }
            Message::SettingsPrefetch(prefetch) => {
                if let Some(ed) = &mut self.settings_editor {
                    ed.prefetch = prefetch;
                }
                Task::none()
            }
            Message::SettingsDarkTheme(dark) => {
                if let Some(ed) = &mut self.settings_editor {
                    ed.dark_theme = dark;
                    ed.sample = sample_model(dark);
                }
                Task::none()
            }
            Message::SettingsWrapLines(wrap) => {
                if let Some(ed) = &mut self.settings_editor {
                    ed.wrap_lines = wrap;
                }
                Task::none()
            }
            Message::SettingsUiFont(name) => {
                if let Some(ed) = &mut self.settings_editor {
                    ed.ui_font = name;
                }
                Task::none()
            }
            Message::SettingsCodeFont(name) => {
                if let Some(ed) = &mut self.settings_editor {
                    ed.code_font = name;
                }
                Task::none()
            }
            Message::SettingsCodeSize(size) => {
                if let Some(ed) = &mut self.settings_editor {
                    ed.code_font_size = size;
                }
                Task::none()
            }
            Message::SettingsUiScale(scale) => {
                if let Some(ed) = &mut self.settings_editor {
                    ed.ui_scale = scale;
                }
                Task::none()
            }
            Message::SaveSettings => {
                let Some(ed) = &mut self.settings_editor else { return Task::none() };
                let command = editor_text(&ed.command).trim().to_string();
                if command.is_empty() {
                    ed.error = Some("The Claude command must not be empty.".into());
                    return Task::none();
                }
                let old = self.settings.clone();
                self.settings.claude_command = command;
                self.settings.orient_prompt = editor_text(&ed.orient_prompt);
                self.settings.general_chat_meta = editor_text(&ed.general_chat_meta);
                self.settings.plan_prompt = editor_text(&ed.plan_prompt);
                self.settings.element_prompt = editor_text(&ed.element_prompt);
                self.settings.element_chat_meta = editor_text(&ed.element_chat_meta);
                self.settings.prefetch = ed.prefetch;
                self.settings.dark_theme = ed.dark_theme;
                self.settings.wrap_lines = ed.wrap_lines;
                self.settings.ui_font = ed.ui_font.trim().to_string();
                self.settings.code_font = ed.code_font.trim().to_string();
                self.settings.code_font_size = (ed.code_font_size * 2.0).round() / 2.0;
                self.settings.ui_scale = (ed.ui_scale * 20.0).round() / 20.0;
                if let Err(e) = self.settings.save() {
                    ed.error = Some(e);
                    return Task::none();
                }
                crate::fonts::set_code(&self.settings.code_font, self.settings.code_font_size);
                if old.ui_font != self.settings.ui_font {
                    // iced only reads the default font at startup: relaunch on the same directory.
                    let relaunched = self.relaunch();
                    match relaunched {
                        Ok(()) => return iced::exit(),
                        Err(e) => {
                            if let Some(ed) = &mut self.settings_editor {
                                ed.error = Some(format!("Settings saved, but restarting failed: {e}. Restart manually to apply the UI font."));
                            }
                            return Task::none();
                        }
                    }
                }
                self.settings_editor = None;
                let mut tasks = Vec::new();
                if let Screen::Walkthrough(w) = &mut self.screen {
                    if old.claude_command != self.settings.claude_command {
                        // The subscription identity changes, which restarts Claude.
                        w.restart_claude("Claude was restarted because its command changed.", &self.settings);
                    }
                    if old.dark_theme != self.settings.dark_theme {
                        tasks.push(rehighlight(w, self.settings.dark_theme));
                    }
                }
                Task::batch(tasks)
            }

            // --------------------------------------------------- directory select
            Message::PickDir => {
                if let Screen::DirSelect(s) = &mut self.screen {
                    s.busy = true;
                }
                Task::perform(
                    async {
                        let started = std::time::Instant::now();
                        let picked = rfd::AsyncFileDialog::new()
                            .set_title("Select a directory")
                            .pick_folder()
                            .await
                            .map(|h| h.path().to_path_buf());
                        // A dialog the user cancelled stays open for a while; an immediate `None`
                        // means no dialog could be shown at all.
                        (picked, started.elapsed() < std::time::Duration::from_millis(400))
                    },
                    |(picked, instant)| Message::DirPicked(picked, instant),
                )
            }
            Message::DirPicked(path, instant) => {
                let Screen::DirSelect(s) = &mut self.screen else { return Task::none() };
                s.busy = false;
                if path.is_none() && instant {
                    s.errors.push(
                        "The system folder picker could not be opened (is xdg-desktop-portal running?). Type or paste the directory path instead.".into(),
                    );
                }
                path.map_or_else(Task::none, |p| self.verify(p))
            }
            Message::DirPathChanged(p) => {
                if let Screen::DirSelect(s) = &mut self.screen {
                    s.path_input = p;
                }
                Task::none()
            }
            Message::OpenDirPath => {
                let Screen::DirSelect(s) = &mut self.screen else { return Task::none() };
                let raw = s.path_input.trim();
                if raw.is_empty() {
                    return Task::none();
                }
                let p = match raw.strip_prefix("~/") {
                    Some(rest) => dirs::home_dir().unwrap_or_default().join(rest),
                    None => PathBuf::from(raw),
                };
                self.verify(p)
            }
            Message::OpenRecent(p) => self.verify(p),
            Message::DirVerified(result) => match result {
                Ok(dir) => self.open_dir(dir),
                Err(e) => {
                    if let Screen::DirSelect(s) = &mut self.screen {
                        s.busy = false;
                        s.errors.push(e);
                    }
                    Task::none()
                }
            },
            Message::DismissError(i) => {
                let errors = match &mut self.screen {
                    Screen::DirSelect(s) => &mut s.errors,
                    Screen::Walkthrough(w) => &mut w.errors,
                };
                if i < errors.len() {
                    errors.remove(i);
                }
                Task::none()
            }
            Message::BackToDirSelect => {
                self.screen = Screen::DirSelect(DirSelect::default());
                Task::none()
            }
            Message::ToggleTree => {
                self.settings.show_tree = !self.settings.show_tree;
                let _ = self.settings.save();
                Task::none()
            }
            Message::CopyText(t) => {
                self.copy_seq += 1;
                let seq = self.copy_seq;
                self.copied = Some((seq, t.clone()));
                Task::batch([
                    iced::clipboard::write(t),
                    Task::perform(tokio::time::sleep(COPIED_FEEDBACK), move |_| Message::CopiedExpired(seq)),
                ])
            }
            Message::CopiedExpired(seq) => {
                if self.copied.as_ref().is_some_and(|(s, _)| *s == seq) {
                    self.copied = None;
                }
                Task::none()
            }

            // --------------------------------------------------------- walkthrough
            other => {
                let Screen::Walkthrough(w) = &mut self.screen else { return Task::none() };
                walkthrough_update(w, other, &self.settings, &mut self.modal_error)
            }
        }
    }
}

fn rehighlight(w: &Walkthrough, dark: bool) -> Task<Message> {
    let inputs: Vec<_> = w.files.iter().enumerate().map(|(i, f)| (i, f.path.clone(), f.text.clone())).collect();
    Task::perform(
        async move {
            tokio::task::spawn_blocking(move || {
                inputs.into_iter().map(|(i, path, text)| (i, highlight::highlight_file(&path, &text, dark))).collect::<Vec<_>>()
            })
            .await
            .unwrap_or_default()
        },
        Message::Rehighlighted,
    )
}

fn walkthrough_update(w: &mut Walkthrough, message: Message, settings: &Settings, modal_error: &mut Option<String>) -> Task<Message> {
    match message {
        Message::ToggleChat => {
            w.show_chat = !w.show_chat;
            Task::none()
        }
        Message::ToggleCollapsed(i) => {
            if let Some(f) = w.files.get_mut(i) {
                f.collapsed = !f.collapsed;
            }
            Task::none()
        }
        Message::ToggleWrap(i) => {
            if let Some(f) = w.files.get_mut(i) {
                f.wrap = Some(!f.wraps(settings.wrap_lines));
            }
            Task::none()
        }
        Message::ExpandAll | Message::CollapseAll => {
            let collapse = matches!(message, Message::CollapseAll);
            for f in &mut w.files {
                f.collapsed = collapse;
            }
            Task::none()
        }
        Message::FilterChanged(s) => {
            w.filter = s;
            Task::none()
        }
        Message::ToggleExt(ext) => {
            if !w.hidden_exts.remove(&ext) {
                w.hidden_exts.insert(ext);
            }
            Task::none()
        }
        Message::ToggleDir(d) => {
            if !w.collapsed_dirs.remove(&d) {
                w.collapsed_dirs.insert(d);
            }
            Task::none()
        }
        Message::JumpToFile(i) => {
            if let Some(f) = w.files.get_mut(i) {
                f.collapsed = false;
            }
            scroll_to_file(i)
        }
        Message::LookingAt(i) => {
            w.looking_at = i;
            Task::none()
        }
        Message::ScrollTo(Some(y)) => operation::scroll_to(PAGE_SCROLL_ID, operation::AbsoluteOffset { x: 0.0, y }),
        Message::ScrollTo(None) => Task::none(),
        Message::LinkClicked(uri) => {
            open_url(&uri.to_string());
            Task::none()
        }
        Message::Rehighlighted(results) => {
            for (i, spans) in results {
                if let Some(f) = w.files.get_mut(i) {
                    f.code.spans = spans;
                }
            }
            Task::none()
        }

        // -------------------------------------------------------- walkthrough loops
        Message::StartFullWalkthrough => {
            w.general_chat.entries.push(ChatEntry::User("Walk me through the full codebase, file by file.".into()));
            start_planning(w, walkthrough::FULL_SCOPE, settings);
            scroll_chat_to_end(Target::General)
        }
        Message::StartPartialWalkthrough => {
            let mut input = text_editor::Content::with_text("Walk me through ");
            input.perform(text_editor::Action::Move(text_editor::Motion::DocumentEnd));
            w.general_chat.input = input;
            operation::focus(GENERAL_CHAT_INPUT_ID)
        }
        Message::FileLoaded(plan_idx, result) => {
            let Some(item) = w.walk().and_then(|walk| walk.plan.get(plan_idx)).cloned() else { return Task::none() };
            match result {
                Ok(file) => {
                    let code = CodeModel { lines: file.lines, spans: file.spans };
                    w.files.push(FileEntry {
                        path: item.path,
                        why: item.why,
                        text: file.text,
                        code,
                        wrap: None,
                        collapsed: false,
                        elements: Vec::new(),
                    });
                    enter_file(w, w.files.len() - 1, settings)
                }
                Err(e) => {
                    w.general_chat.entries.push(ChatEntry::Note(format!("Skipped {}: {e}.", item.path)));
                    let step = w.walk_mut().map_or(Step::Wait, Walk::skip_file);
                    Task::batch([scroll_chat_to_end(Target::General), run_step(w, step, settings)])
                }
            }
        }
        Message::ToggleElement(i, e) => {
            if w.active == Some((i, e)) {
                return Task::none();
            }
            let Some(el) = w.files.get_mut(i).and_then(|f| f.elements.get_mut(e)) else { return Task::none() };
            el.expanded = !el.expanded;
            if el.expanded {
                Task::batch([reveal_element_chat(i, e, false), operation::focus(element_chat_input_id(i, e))])
            } else {
                Task::none()
            }
        }
        Message::ThumbsUp => {
            let Some((i, e)) = w.active.take() else { return Task::none() };
            if let Some(el) = w.files.get_mut(i).and_then(|f| f.elements.get_mut(e)) {
                el.expanded = false;
            }
            let step = w.walk_mut().map_or(Step::Wait, Walk::next);
            run_step(w, step, settings)
        }
        Message::RetryElement => {
            let step = w.walk_mut().map_or(Step::Wait, Walk::next);
            run_step(w, step, settings)
        }

        // ------------------------------------------------------------------ chats
        Message::ElementChatEdit(i, e, action) => {
            if let Some(c) = w.chat_mut(Target::Element(i, e)) {
                c.input.perform(action);
            }
            Task::none()
        }
        Message::GeneralChatEdit(action) => {
            w.general_chat.input.perform(action);
            Task::none()
        }
        Message::ElementChatSend(i, e) => send_chat(w, Target::Element(i, e), settings),
        Message::GeneralChatSend => send_chat(w, Target::General, settings),

        // ----------------------------------------------------------------- Claude
        Message::StopClaude => {
            if let (Some(tx), Some(_)) = (&w.claude.sender, &w.claude.in_flight) {
                let _ = tx.send(Outgoing::Interrupt);
                w.claude.activity = "interrupting…".into();
                w.claude.interrupting = true;
            }
            Task::none()
        }
        Message::RestartClaude => {
            w.restart_claude("Claude was restarted.", settings);
            Task::none()
        }
        Message::Claude(event) => claude_event(w, event, settings, modal_error),
        Message::PermissionDecision { allow, always } => {
            if let Some(p) = w.claude.permissions.pop_front() {
                if let Some(tx) = &w.claude.sender {
                    let _ = tx.send(Outgoing::Permission {
                        request_id: p.request_id,
                        allow,
                        input: p.input,
                        updated_permissions: if always { p.suggestions } else { None },
                    });
                }
                w.claude.activity = if allow { format!("running {}…", p.tool_name) } else { "permission denied".into() };
            }
            Task::none()
        }
        _ => Task::none(),
    }
}

fn open_url(url: &str) {
    let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    if let Err(e) = std::process::Command::new(opener).arg(url).spawn() {
        eprintln!("could not open {url}: {e}");
    }
}

/// Silently asks Claude for the file order.
fn start_planning(w: &mut Walkthrough, scope: &str, settings: &Settings) {
    if matches!(w.phase, Phase::Planning | Phase::Walking(_)) {
        return;
    }
    w.phase = Phase::Planning;
    let vars = PromptVars { scope: scope.to_string(), ..w.base_vars() };
    let prompt = settings::render_template(&settings.plan_prompt, &vars, "");
    w.enqueue(ClaudeJob { target: Target::Plan, prompt });
}

/// Takes Claude's file order and starts the file loop.
fn accept_plan(w: &mut Walkthrough, text: &str, settings: &Settings) -> Task<Message> {
    let parsed = walkthrough::parse_plan(text).map(|items| walkthrough::check_plan(items, &w.dir.path));
    let (plan, dropped) = match parsed {
        Ok(result) if !result.0.is_empty() => result,
        Ok(_) => {
            w.phase = Phase::Chat;
            w.general_chat.entries.push(ChatEntry::Error("Claude's plan has no files that can be shown.".into()));
            return Task::none();
        }
        Err(e) => {
            eprintln!("[plan] could not parse response:\n{text}");
            w.phase = Phase::Chat;
            w.general_chat.entries.push(ChatEntry::Error(e));
            return Task::none();
        }
    };
    let list: String = plan
        .iter()
        .enumerate()
        .map(|(n, item)| {
            let why = if item.why.is_empty() { String::new() } else { format!(": {}", item.why) };
            format!("{}. `{}`{why}\n", n + 1, item.path)
        })
        .collect();
    let summary = format!("Here's the plan:\n\n{list}");
    w.general_chat.entries.push(ChatEntry::Assistant { msg_id: String::new(), md: markdown::Content::parse(&summary), text: summary });
    if !dropped.is_empty() {
        w.general_chat.entries.push(ChatEntry::Note(format!("Left out of the plan because they aren't files here: {}", dropped.join(", "))));
    }
    let mut walk = Walk::new(plan);
    let step = walk.start();
    w.phase = Phase::Walking(walk);
    w.file = None;
    w.active = None;
    run_step(w, step, settings)
}

/// Carries out what the walkthrough asks for.
fn run_step(w: &mut Walkthrough, step: Step, settings: &Settings) -> Task<Message> {
    match step {
        Step::Wait => Task::none(),
        Step::Fetch(fetch) => {
            w.enqueue_fetch(fetch, settings);
            Task::none()
        }
        Step::Show(element) => show_element(w, element, settings),
        Step::Load(plan_idx) => {
            let Some(path) = w.walk().and_then(|walk| walk.plan.get(plan_idx)).map(|item| item.path.clone()) else {
                return Task::none();
            };
            // A file from an earlier walkthrough is on the page already.
            if let Some(i) = w.files.iter().position(|f| f.path == path) {
                return enter_file(w, i, settings);
            }
            Task::perform(files::load_file(w.dir.path.clone(), path, settings.dark_theme), move |r| Message::FileLoaded(plan_idx, r))
        }
        Step::Finished => {
            w.phase = Phase::Finished;
            w.file = None;
            w.general_chat.entries.push(ChatEntry::Note("That's the end of the walkthrough. Feel free to keep asking questions.".into()));
            scroll_chat_to_end(Target::General)
        }
    }
}

/// Loop a: the file is on the page; shows its top and starts on its elements.
fn enter_file(w: &mut Walkthrough, i: usize, settings: &Settings) -> Task<Message> {
    w.file = Some(i);
    w.earlier_elements = w.files[i].elements.len();
    w.files[i].collapsed = false;
    let step = w.walk_mut().map_or(Step::Wait, Walk::on_file_ready);
    let next = run_step(w, step, settings);
    // In this order: when the first element is ready right away, it decides the position.
    scroll_to_file(i).chain(next)
}

/// Loop b: shows a selection with Claude's explanation.
fn show_element(w: &mut Walkthrough, element: Element, settings: &Settings) -> Task<Message> {
    let Some(i) = w.file else { return Task::none() };
    let Element { start, end, comment } = element;
    let f = &mut w.files[i];
    let count = f.code.lines.len() as u32;
    if start == 0 || start > count {
        let reason = format!("Claude selected line {start}, but {} has {count} lines.", f.path);
        if let Some(walk) = w.walk_mut() {
            walk.fail(reason);
        }
        return Task::none();
    }
    let end = end.clamp(start, count);
    f.collapsed = false;
    f.elements.push(ElementEntry::new(start, end, comment));
    let e = f.elements.len() - 1;
    w.active = Some((i, e));
    w.maybe_prefetch(settings);
    present_element(i, e)
}

fn send_chat(w: &mut Walkthrough, target: Target, settings: &Settings) -> Task<Message> {
    let (template, vars) = match target {
        Target::Element(i, e) => {
            let Some((f, el)) = w.files.get(i).and_then(|f| Some((f, f.elements.get(e)?))) else { return Task::none() };
            let vars = PromptVars { path: f.path.clone(), lines: el.lines_label(), ..w.base_vars() };
            (&settings.element_chat_meta, vars)
        }
        _ => (&settings.general_chat_meta, w.base_vars()),
    };
    let Some(chat) = w.chat_mut(target) else { return Task::none() };
    let text = editor_text(&chat.input).trim().to_string();
    if text.is_empty() {
        return Task::none();
    }
    chat.input = text_editor::Content::new();
    // The user's own message is shown; the meta prompt wrapped around it is not.
    chat.entries.push(ChatEntry::User(text.clone()));
    let prompt = settings::render_template(template, &vars, &text);
    w.cancel_prefetch();
    w.enqueue(ClaudeJob { target, prompt });
    scroll_chat_to_end(target)
}

fn assistant_entry<'a>(chat: &'a mut Chat, msg_id: &str) -> &'a mut ChatEntry {
    let pos = chat
        .entries
        .iter()
        .rposition(|e| matches!(e, ChatEntry::Assistant { msg_id: id, .. } if id == msg_id));
    // Only reuse the entry if it is the last one (otherwise tool calls happened in between).
    match pos {
        Some(p) if p + 1 == chat.entries.len() => &mut chat.entries[p],
        _ => {
            chat.entries.push(ChatEntry::Assistant {
                msg_id: msg_id.to_string(),
                text: String::new(),
                md: markdown::Content::new(),
            });
            chat.entries.last_mut().expect("just pushed")
        }
    }
}

/// Adds text to an assistant message. Chat panel messages may hold the start directive,
/// which never gets shown.
fn append_assistant(entry: &mut ChatEntry, text: &str, target: Target) {
    let ChatEntry::Assistant { text: buf, md, .. } = entry else { return };
    buf.push_str(text);
    if target.in_panel() {
        *md = markdown::Content::parse(&walkthrough::split_directive(buf).0);
    } else {
        md.push_str(text);
    }
}

/// Removes the start directive from the reply that just finished, and returns its scope.
fn take_directive(chat: &mut Chat) -> Option<String> {
    let ChatEntry::Assistant { text, md, .. } = chat.entries.iter_mut().rev().find(|e| matches!(e, ChatEntry::Assistant { .. }))? else {
        return None;
    };
    let (visible, scope) = walkthrough::split_directive(text);
    let scope = scope?;
    *md = markdown::Content::parse(&visible);
    *text = visible;
    Some(scope)
}

fn claude_event(w: &mut Walkthrough, event: claude::Event, settings: &Settings, modal_error: &mut Option<String>) -> Task<Message> {
    use claude::Event as E;
    let target = w.claude.in_flight.as_ref().map(|j| j.target);
    match event {
        E::Started(tx) => {
            w.claude.sender = Some(tx);
            w.claude.status = ClaudeStatus::Ready;
            w.pump();
            Task::none()
        }
        E::Init { session_id, model, permission_mode } => {
            w.claude.session_id = session_id;
            w.claude.model = model;
            w.claude.permission_mode = permission_mode;
            Task::none()
        }
        E::MessageStart { id } => {
            w.claude.current_msg = id;
            w.claude.activity = "writing…".into();
            Task::none()
        }
        E::TextDelta { text } => {
            let msg_id = w.claude.current_msg.clone();
            if let Some(t) = target.filter(|t| t.is_chat())
                && let Some(chat) = w.chat_mut(t)
            {
                append_assistant(assistant_entry(chat, &msg_id), &text, t);
                return scroll_chat_to_end(t);
            }
            Task::none()
        }
        E::AssistantText { id, text } => {
            let Some(t) = target else { return Task::none() };
            let Some(chat) = w.chat_mut(t) else { return Task::none() };
            if !t.is_chat() {
                // The JSON answer is for the tool, but remarks along the way are worth showing.
                if !text.contains("{\"") && !text.trim().is_empty() {
                    chat.entries.push(ChatEntry::Note(text));
                    return scroll_chat_to_end(t);
                }
                return Task::none();
            }
            let streamed = chat
                .entries
                .iter()
                .any(|e| matches!(e, ChatEntry::Assistant { msg_id, text, .. } if *msg_id == id && !text.is_empty()));
            if !streamed {
                append_assistant(assistant_entry(chat, &id), &text, t);
                return scroll_chat_to_end(t);
            }
            Task::none()
        }
        E::ToolUse { name, input, .. } => {
            let summary = claude::tool_summary(&name, &input);
            w.claude.activity = format!("using {summary}");
            if let Some(t) = target
                && let Some(chat) = w.chat_mut(t)
            {
                chat.entries.push(ChatEntry::Tool(summary));
                return scroll_chat_to_end(t);
            }
            Task::none()
        }
        E::ToolResult { is_error, content, .. } => {
            if is_error
                && let Some(t) = target
                && let Some(chat) = w.chat_mut(t)
            {
                let first = content.lines().next().unwrap_or_default().chars().take(200).collect::<String>();
                chat.entries.push(ChatEntry::Tool(format!("✗ {first}")));
            }
            w.claude.activity = "thinking…".into();
            Task::none()
        }
        E::Result { is_error, subtype, text, cost_usd } => {
            if let Some(c) = cost_usd {
                w.claude.cost = c;
            }
            w.claude.activity.clear();
            let interrupted = std::mem::take(&mut w.claude.interrupting);
            let Some(job) = w.claude.in_flight.take() else { return Task::none() };
            let error = is_error.then(|| if text.is_empty() { subtype } else { text.clone() });
            let task = job_finished(w, job.target, &text, interrupted, error, settings);
            w.pump();
            w.maybe_prefetch(settings);
            task
        }
        E::Permission(p) => {
            w.claude.activity = format!("waiting for permission: {}", p.tool_name);
            w.claude.permissions.push_back(p);
            Task::none()
        }
        E::CancelPermission { request_id } => {
            w.claude.permissions.retain(|p| p.request_id != request_id);
            Task::none()
        }
        E::Exited(msg) => {
            w.claude.sender = None;
            w.claude.status = ClaudeStatus::Exited(msg.clone());
            w.claude.permissions.clear();
            w.fail_all_jobs("The Claude process exited.");
            *modal_error = Some(format!("{msg}\n\nCheck the Claude command in Settings, then use \"Restart\" in the status bar."));
            Task::none()
        }
    }
}

fn job_finished(w: &mut Walkthrough, target: Target, text: &str, interrupted: bool, error: Option<String>, settings: &Settings) -> Task<Message> {
    let failure = if interrupted { Some("Interrupted.".to_string()) } else { error.map(|e| format!("Claude: {e}")) };
    match target {
        Target::Fetch(fetch) => {
            let reply = match failure {
                Some(reason) => Err(reason),
                None => walkthrough::parse_reply(text),
            };
            let Some(walk) = w.walk_mut() else { return Task::none() };
            match reply {
                Ok(reply) => {
                    let step = walk.on_reply(fetch.ticket, reply);
                    run_step(w, step, settings)
                }
                Err(reason) => {
                    eprintln!("[element] no usable answer: {reason}\n{text}");
                    walk.on_failure(fetch.ticket, &reason);
                    Task::none()
                }
            }
        }
        Target::Plan => match failure {
            Some(reason) => {
                w.finish_job_with_error(target, &reason);
                scroll_chat_to_end(target)
            }
            None => Task::batch([accept_plan(w, text, settings), scroll_chat_to_end(target)]),
        },
        t => {
            if t == Target::Orient {
                w.phase = Phase::Chat;
                w.oriented = failure.is_none();
            }
            let Some(chat) = w.chat_mut(t) else { return Task::none() };
            if interrupted {
                chat.entries.push(ChatEntry::Tool("■ interrupted".into()));
            } else if let Some(reason) = failure {
                chat.entries.push(ChatEntry::Error(reason));
            } else if !chat.entries.iter().rev().take_while(|e| !matches!(e, ChatEntry::User(_))).any(|e| matches!(e, ChatEntry::Assistant { .. })) && !text.is_empty() {
                // No streamed text arrived (e.g. streaming disabled): show the final result.
                let entry = assistant_entry(chat, "");
                append_assistant(entry, text, t);
            }
            if t == Target::General
                && let Some(scope) = take_directive(chat)
            {
                start_planning(w, &walkthrough::scope_text(&scope), settings);
            }
            scroll_chat_to_end(t)
        }
    }
}
