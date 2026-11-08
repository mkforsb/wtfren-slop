//! The walkthrough itself, free of any UI: parsing Claude's structured answers,
//! and deciding what happens next in the file loop and the per-file element loop.
//!
//! The application feeds [`Walk`] what happened (the user moved on, a file loaded,
//! Claude answered) and carries out the [`Step`]s it gets back.

use serde_json::Value;
use std::collections::{HashSet, VecDeque};
use std::path::Path;

// ---------------------------------------------------------------------------
// Claude's answers
// ---------------------------------------------------------------------------

/// How Claude tells the tool, in a chat reply, that the user wants to start the
/// walkthrough: `[[START_WALKTHROUGH: <scope>]]`.
const START_TAG: &str = "[[START_WALKTHROUGH:";

/// A chat reply without the start directive, and the directive's scope if there was one.
///
/// Also hides what may be the beginning of the directive at the end of the text, so
/// it doesn't flash by while a reply streams in.
pub fn split_directive(raw: &str) -> (String, Option<String>) {
    if let Some(start) = raw.find(START_TAG) {
        let rest = &raw[start + START_TAG.len()..];
        let (scope, after) = match rest.find("]]") {
            Some(end) => (Some(rest[..end].trim().to_string()), &rest[end + 2..]),
            None => (None, ""),
        };
        let visible = format!("{}{after}", &raw[..start]).trim_end().to_string();
        return (visible, scope.filter(|s| !s.is_empty()));
    }
    let partial = (2..START_TAG.len()).rev().find(|&n| raw.ends_with(&START_TAG[..n]));
    let end = raw.len() - partial.unwrap_or(0);
    (raw[..end].to_string(), None)
}

/// What the scope in a start directive means, in words for the planning prompt.
pub fn scope_text(scope: &str) -> String {
    if scope.eq_ignore_ascii_case("full") { FULL_SCOPE.to_string() } else { scope.to_string() }
}

pub const FULL_SCOPE: &str = "the full codebase";

/// The first JSON object in `text` that `accept` likes. Claude is told to answer with
/// JSON only, but may still wrap it in prose or a code fence.
fn find_json(text: &str, accept: impl Fn(&Value) -> bool) -> Option<Value> {
    text.match_indices('{').find_map(|(i, _)| {
        let value = serde_json::Deserializer::from_str(&text[i..]).into_iter::<Value>().next()?.ok()?;
        accept(&value).then_some(value)
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanItem {
    pub path: String,
    /// Why the file comes at this point of the walkthrough.
    pub why: String,
}

pub fn parse_plan(text: &str) -> Result<Vec<PlanItem>, String> {
    let v = find_json(text, |v| v["files"].is_array()).ok_or("Claude's answer has no JSON object with a \"files\" list.")?;
    let items = v["files"].as_array().map(Vec::as_slice).unwrap_or_default();
    Ok(items
        .iter()
        .filter_map(|item| {
            let path = item.as_str().or(item["path"].as_str())?.trim();
            (!path.is_empty()).then(|| PlanItem { path: path.to_string(), why: item["why"].as_str().unwrap_or_default().trim().to_string() })
        })
        .collect())
}

/// Makes the plan's paths relative to `root`, and drops the ones that can't be shown:
/// duplicates, and anything that isn't a file inside `root`. Also returns the dropped paths.
pub fn check_plan(items: Vec<PlanItem>, root: &Path) -> (Vec<PlanItem>, Vec<String>) {
    let mut seen = HashSet::new();
    let (mut plan, mut dropped) = (Vec::new(), Vec::new());
    for mut item in items {
        let relative = Path::new(&item.path).strip_prefix(root).map(|p| p.to_string_lossy().into_owned());
        let path = relative.unwrap_or_else(|_| item.path.clone());
        let path = path.trim_start_matches("./").to_string();
        let inside = root.join(&path).canonicalize().is_ok_and(|p| p.starts_with(root) && p.is_file());
        if !inside {
            dropped.push(item.path);
        } else if seen.insert(path.clone()) {
            item.path = path;
            plan.push(item);
        }
    }
    (plan, dropped)
}

/// A selection of lines and Claude's explanation of them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Element {
    /// First and last selected line, 1-based.
    pub start: u32,
    pub end: u32,
    pub comment: String,
}

/// Claude's answer to "what's next in this file?".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    Element(Element),
    /// Nothing more in this file is worth covering.
    Done,
}

pub fn parse_reply(text: &str) -> Result<Reply, String> {
    let v = find_json(text, |v| v.get("start_line").is_some() || v["done"] == true)
        .ok_or("Claude's answer has no JSON object with a walkthrough element.")?;
    let Some(start) = v["start_line"].as_u64() else { return Ok(Reply::Done) };
    let end = v["end_line"].as_u64().unwrap_or(start);
    let comment = v["comment"].as_str().unwrap_or_default().trim();
    if comment.is_empty() {
        return Err("Claude's walkthrough element has no explanation.".into());
    }
    Ok(Reply::Element(Element { start: start as u32, end: end as u32, comment: comment.to_string() }))
}

/// "lines 1-4, 9" for the walkthrough prompt.
pub fn covered_text(ranges: impl IntoIterator<Item = (u32, u32)>) -> String {
    let parts: Vec<String> =
        ranges.into_iter().map(|(a, b)| if a == b { a.to_string() } else { format!("{a}-{b}") }).collect();
    if parts.is_empty() { "nothing yet".into() } else { format!("lines {}", parts.join(", ")) }
}

// ---------------------------------------------------------------------------
// The loops
// ---------------------------------------------------------------------------

/// A request to Claude for the next element of a plan entry. Only the answer to the
/// newest fetch counts; see [`Walk::on_reply`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fetch {
    pub plan_idx: usize,
    pub ticket: u64,
}

/// What the application should do next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Nothing for now.
    Wait,
    /// Ask Claude for an element.
    Fetch(Fetch),
    /// Show this element of the current file.
    Show(Element),
    /// Add the file of this plan entry to the page, then call [`Walk::on_file_ready`].
    Load(usize),
    /// The last file is done.
    Finished,
}

#[derive(Debug)]
pub struct Walk {
    pub plan: Vec<PlanItem>,
    /// The plan entry being walked through.
    pub current: usize,
    /// The file of the current entry isn't on the page yet.
    pub loading: bool,
    /// The user has nothing left to look at until Claude answers.
    pub waiting: bool,
    /// Why the walkthrough stopped; the user can retry.
    pub failed: Option<String>,
    /// A prefetch failed or was stopped: no more of them until the user moves on, so a
    /// Claude that can't answer doesn't get asked again and again in the background.
    prefetch_held: bool,
    /// The request Claude is working on or has queued.
    fetching: Option<Fetch>,
    /// Answers that arrived before the user asked for them, oldest first.
    ready: VecDeque<(usize, Reply)>,
    last_ticket: u64,
}

impl Walk {
    pub fn new(plan: Vec<PlanItem>) -> Self {
        Self { plan, current: 0, loading: false, waiting: false, failed: None, prefetch_held: false, fetching: None, ready: VecDeque::new(), last_ticket: 0 }
    }

    /// Enters the file loop.
    pub fn start(&mut self) -> Step {
        if self.plan.is_empty() {
            return Step::Finished;
        }
        self.loading = true;
        Step::Load(0)
    }

    fn fetch(&mut self, plan_idx: usize) -> Fetch {
        self.last_ticket += 1;
        let fetch = Fetch { plan_idx, ticket: self.last_ticket };
        self.fetching = Some(fetch);
        fetch
    }

    fn next_file(&mut self) -> Step {
        self.current += 1;
        if self.current >= self.plan.len() {
            return Step::Finished;
        }
        self.loading = true;
        Step::Load(self.current)
    }

    fn consume(&mut self, reply: Reply) -> Step {
        match reply {
            Reply::Done => self.next_file(),
            Reply::Element(e) => Step::Show(e),
        }
    }

    /// The user is ready for the next element of the current file (or asked to retry).
    pub fn next(&mut self) -> Step {
        self.failed = None;
        self.prefetch_held = false;
        let current = self.current;
        self.ready.retain(|(i, _)| *i >= current);
        if self.ready.front().is_some_and(|(i, _)| *i == current) {
            let (_, reply) = self.ready.pop_front().expect("checked above");
            return self.consume(reply);
        }
        self.waiting = true;
        if self.fetching.is_some_and(|f| f.plan_idx == current) {
            return Step::Wait;
        }
        Step::Fetch(self.fetch(current))
    }

    /// The current file is on the page: enter the element loop.
    pub fn on_file_ready(&mut self) -> Step {
        self.loading = false;
        self.next()
    }

    /// The current file can't be shown: move on to the next one.
    pub fn skip_file(&mut self) -> Step {
        self.loading = false;
        self.next_file()
    }

    pub fn on_reply(&mut self, ticket: u64, reply: Reply) -> Step {
        let Some(fetch) = self.fetching.filter(|f| f.ticket == ticket) else { return Step::Wait };
        self.fetching = None;
        if self.waiting && fetch.plan_idx == self.current {
            self.waiting = false;
            return self.consume(reply);
        }
        self.ready.push_back((fetch.plan_idx, reply));
        Step::Wait
    }

    /// A fetch failed. That only matters if the user is waiting for it: a failed
    /// prefetch is simply repeated when its answer is needed.
    pub fn on_failure(&mut self, ticket: u64, reason: &str) {
        if self.fetching.is_some_and(|f| f.ticket == ticket) {
            self.fetching = None;
            if std::mem::take(&mut self.waiting) {
                self.failed = Some(reason.to_string());
            } else {
                self.prefetch_held = true;
            }
        }
    }

    /// An answer turned out to be unusable after it was accepted.
    pub fn fail(&mut self, reason: String) {
        self.failed = Some(reason);
    }

    /// While the user looks at an element: what to ask Claude for ahead of time. That's
    /// the next element of the current file, or the first one of the next file once
    /// Claude has said that the current file is done.
    pub fn prefetch(&mut self) -> Option<Fetch> {
        if self.fetching.is_some() || self.waiting || self.loading || self.failed.is_some() || self.prefetch_held {
            return None;
        }
        let plan_idx = match self.ready.back() {
            None => self.current,
            Some((_, Reply::Element(_))) => return None,
            Some((i, Reply::Done)) => i + 1,
        };
        (plan_idx < self.plan.len()).then(|| self.fetch(plan_idx))
    }

    /// The user said something to Claude, which may change what comes next, so whatever
    /// was fetched ahead of time goes away. Returns the fetch to cancel, if any.
    /// A fetch the user is waiting for stays.
    pub fn invalidate_prefetch(&mut self) -> Option<Fetch> {
        if self.waiting {
            return None;
        }
        self.ready.clear();
        self.fetching.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(n: usize) -> Vec<PlanItem> {
        (0..n).map(|i| PlanItem { path: format!("f{i}"), why: String::new() }).collect()
    }

    fn el(start: u32) -> Element {
        Element { start, end: start, comment: "c".into() }
    }

    fn fetched(step: Step) -> Fetch {
        match step {
            Step::Fetch(f) => f,
            other => panic!("expected a fetch, got {other:?}"),
        }
    }

    #[test]
    fn directive_is_hidden() {
        assert_eq!(split_directive("Sure!\n\n[[START_WALKTHROUGH: full]]\n"), ("Sure!".to_string(), Some("full".to_string())));
        assert_eq!(split_directive("[[START_WALKTHROUGH: the  lexer ]]"), (String::new(), Some("the  lexer".to_string())));
        assert_eq!(split_directive("no directive [here]"), ("no directive [here]".to_string(), None));
        // While streaming: an unfinished directive, and what may become one.
        assert_eq!(split_directive("Ok [[START_WALKTHROUGH: fu"), ("Ok".to_string(), None));
        assert_eq!(split_directive("Ok [[START_W"), ("Ok ".to_string(), None));
        assert_eq!(split_directive("v[0]"), ("v[0]".to_string(), None));
    }

    #[test]
    fn plan_json() {
        let text = "Here {you} go:\n```json\n{\"files\": [{\"path\": \"a.rs\", \"why\": \" entry \"}, \"b.rs\", {\"why\": \"no path\"}]}\n```\nDone {ok}.";
        let p = parse_plan(text).unwrap();
        assert_eq!(p, [PlanItem { path: "a.rs".into(), why: "entry".into() }, PlanItem { path: "b.rs".into(), why: String::new() }]);
        assert!(parse_plan("no plan").is_err());
    }

    #[test]
    fn plan_paths() {
        let dir = std::env::temp_dir().join(format!("walkthrough-plan-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("sub/a.rs"), "x").unwrap();
        std::fs::write(dir.join("outside.rs"), "x").unwrap();
        let root = dir.join("sub").canonicalize().unwrap();
        let item = |p: &str| PlanItem { path: p.into(), why: String::new() };
        let absolute = root.join("a.rs").display().to_string();
        let (plan, dropped) = check_plan(vec![item("./a.rs"), item(&absolute), item("missing.rs"), item("../outside.rs")], &root);
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(plan, [item("a.rs")]);
        assert_eq!(dropped, ["missing.rs", "../outside.rs"]);
    }

    #[test]
    fn reply_json() {
        let r = parse_reply("{\"start_line\": 3, \"end_line\": 5, \"comment\": \"Uses `{x}`\"}").unwrap();
        assert_eq!(r, Reply::Element(Element { start: 3, end: 5, comment: "Uses `{x}`".into() }));
        assert_eq!(parse_reply("All covered. {\"done\": true}").unwrap(), Reply::Done);
        assert!(parse_reply("{\"start_line\": 3}").is_err());
        assert!(parse_reply("{\"done\": false}").is_err());
        assert!(parse_reply("nope").is_err());
    }

    #[test]
    fn covered() {
        assert_eq!(covered_text([]), "nothing yet");
        assert_eq!(covered_text([(1, 4), (9, 9)]), "lines 1-4, 9");
    }

    #[test]
    fn walks_all_files_without_prefetching() {
        let mut w = Walk::new(plan(2));
        assert_eq!(w.start(), Step::Load(0));
        let f = fetched(w.on_file_ready());
        assert!(w.waiting);
        assert_eq!(w.on_reply(f.ticket, Reply::Element(el(1))), Step::Show(el(1)));
        assert!(!w.waiting);
        let f = fetched(w.next());
        assert_eq!(w.on_reply(f.ticket, Reply::Done), Step::Load(1));
        let f = fetched(w.on_file_ready());
        assert_eq!(f.plan_idx, 1);
        assert_eq!(w.on_reply(f.ticket, Reply::Done), Step::Finished);
    }

    #[test]
    fn an_empty_plan_finishes_at_once() {
        assert_eq!(Walk::new(plan(0)).start(), Step::Finished);
    }

    #[test]
    fn prefetched_element_is_shown_without_waiting() {
        let mut w = Walk::new(plan(1));
        w.start();
        let f = fetched(w.on_file_ready());
        w.on_reply(f.ticket, Reply::Element(el(1)));
        let f = w.prefetch().unwrap();
        assert_eq!(w.prefetch(), None, "one request at a time");
        assert_eq!(w.on_reply(f.ticket, Reply::Element(el(5))), Step::Wait);
        assert_eq!(w.prefetch(), None, "one element ahead is enough");
        assert_eq!(w.next(), Step::Show(el(5)));
    }

    #[test]
    fn moving_on_during_a_prefetch_waits_for_it() {
        let mut w = Walk::new(plan(1));
        w.start();
        let f = fetched(w.on_file_ready());
        w.on_reply(f.ticket, Reply::Element(el(1)));
        let f = w.prefetch().unwrap();
        assert_eq!(w.next(), Step::Wait);
        assert!(w.waiting);
        assert_eq!(w.on_reply(f.ticket, Reply::Element(el(5))), Step::Show(el(5)));
    }

    #[test]
    fn prefetch_continues_into_the_next_file() {
        let mut w = Walk::new(plan(2));
        w.start();
        let f = fetched(w.on_file_ready());
        w.on_reply(f.ticket, Reply::Element(el(1)));
        let f = w.prefetch().unwrap();
        w.on_reply(f.ticket, Reply::Done);
        let f = w.prefetch().unwrap();
        assert_eq!(f.plan_idx, 1);
        w.on_reply(f.ticket, Reply::Element(el(2)));
        assert_eq!(w.next(), Step::Load(1));
        assert_eq!(w.on_file_ready(), Step::Show(el(2)));
    }

    #[test]
    fn prefetch_stops_after_the_last_file() {
        let mut w = Walk::new(plan(1));
        w.start();
        let f = fetched(w.on_file_ready());
        w.on_reply(f.ticket, Reply::Element(el(1)));
        let f = w.prefetch().unwrap();
        w.on_reply(f.ticket, Reply::Done);
        assert_eq!(w.prefetch(), None);
        assert_eq!(w.next(), Step::Finished);
    }

    #[test]
    fn talking_to_claude_drops_what_was_prefetched() {
        let mut w = Walk::new(plan(1));
        w.start();
        let f = fetched(w.on_file_ready());
        w.on_reply(f.ticket, Reply::Element(el(1)));

        // An answer that already arrived.
        let f = w.prefetch().unwrap();
        w.on_reply(f.ticket, Reply::Element(el(5)));
        assert_eq!(w.invalidate_prefetch(), None);

        // A request still underway: its late answer is ignored.
        let f = w.prefetch().unwrap();
        assert_eq!(w.invalidate_prefetch(), Some(f));
        assert_eq!(w.on_reply(f.ticket, Reply::Element(el(5))), Step::Wait);

        let f = fetched(w.next());
        assert_eq!(w.on_reply(f.ticket, Reply::Element(el(7))), Step::Show(el(7)));
    }

    #[test]
    fn a_fetch_the_user_waits_for_is_not_invalidated() {
        let mut w = Walk::new(plan(1));
        w.start();
        let f = fetched(w.on_file_ready());
        assert_eq!(w.invalidate_prefetch(), None);
        assert_eq!(w.on_reply(f.ticket, Reply::Element(el(1))), Step::Show(el(1)));
    }

    #[test]
    fn failures() {
        let mut w = Walk::new(plan(1));
        w.start();
        let f = fetched(w.on_file_ready());
        w.on_failure(f.ticket, "boom");
        assert_eq!(w.failed.as_deref(), Some("boom"));
        assert_eq!(w.prefetch(), None, "nothing happens until the user retries");
        let f = fetched(w.next());
        assert_eq!(w.failed, None);
        w.on_reply(f.ticket, Reply::Element(el(1)));

        // A failed prefetch goes unnoticed, isn't repeated right away, and is repeated
        // when its answer is needed.
        let f = w.prefetch().unwrap();
        w.on_failure(f.ticket, "boom");
        assert_eq!(w.failed, None);
        assert_eq!(w.prefetch(), None);
        let f = fetched(w.next());
        w.on_reply(f.ticket, Reply::Element(el(3)));
        assert!(w.prefetch().is_some(), "prefetching resumes once the user has moved on");
    }

    #[test]
    fn unshowable_files_are_skipped() {
        let mut w = Walk::new(plan(2));
        w.start();
        assert_eq!(w.skip_file(), Step::Load(1));
        assert_eq!(w.skip_file(), Step::Finished);
    }
}
