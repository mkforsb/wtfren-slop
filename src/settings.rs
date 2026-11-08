//! Persistent application settings and prompt templates.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Walking through code shouldn't change it, and line numbers must stay valid, so Claude
/// doesn't get the editing tools. Read-only git commands don't need a permission prompt.
pub const DEFAULT_CLAUDE_COMMAND: &str = "claude -p --input-format stream-json --output-format stream-json --verbose --include-partial-messages --permission-prompt-tool stdio --permission-mode default --disallowedTools AskUserQuestion Edit Write NotebookEdit --allowedTools 'Bash(git log *)' 'Bash(git show *)' 'Bash(git blame *)' 'Bash(git diff *)' 'Bash(git ls-files *)' 'Bash(ls *)'";

pub const DEFAULT_ORIENT_PROMPT: &str = "[Meta: this message comes from a codebase walkthrough tool, not from the user. The user doesn't see it. They only see your tool calls and your reply, in a chat panel.]\n\nThe user pointed the tool at the directory {dir}. {git_info}\n\nGet somewhat briefly oriented in the codebase: look at the directory layout, the README and the build or manifest files, and skim a few key files. Keep it short: a handful of tool calls, not a full read. Do NOT edit any files.\n\nThen give the user a brief summary of what this codebase is and how it is organized: one or two short paragraphs, with no review of the code, since the details can wait. Then ask whether they would like to keep chatting about the codebase in general or jump straight into a file-by-file walkthrough, either of the full codebase or of specific parts of it. If the directory doesn't contain any code, simply say that there's no code to walk through.";

pub const DEFAULT_GENERAL_CHAT_META: &str = "[Meta: the following message was entered into the general chat panel of a codebase walkthrough tool. Walkthrough status: {status}.\n\nIf no walkthrough is in progress and the user asks to go ahead with the file-by-file walkthrough, reply with a very short acknowledgement and end your reply with a line of exactly this form: [[START_WALKTHROUGH: <scope>]] where <scope> is `full` for the full codebase, or else a short description of the part or parts to cover. The tool hides that line and takes it from there, so don't plan the walkthrough in this reply. Never write that line for any other reason.]\n\n{message}";

pub const DEFAULT_PLAN_PROMPT: &str = "[Meta: this message comes from a codebase walkthrough tool, not from the user. Your answer is parsed by a program, not read by a human.]\n\nThe user wants a file-by-file walkthrough of {scope}. Decide which files to go through, and in which order. The order should make the code easy to learn: take into account directory order, submodule order, the order in which concepts build on each other, and entry points versus the things they rely on. Leave out files with nothing worth explaining, such as lock files, generated code, vendored code and binary assets. Use tools if you need to look at more of the code. Do NOT edit any files.\n\nRespond with ONLY a JSON object, no prose and no code fences, of the form:\n{\"files\": [{\"path\": \"src/main.rs\", \"why\": \"one short line on why this file comes at this point\"}]}\nwhere the paths are relative to {dir}.";

pub const DEFAULT_ELEMENT_PROMPT: &str = "[Meta: this message comes from a codebase walkthrough tool, not from the user. Your answer is parsed by a program, not read by a human.]\n\nThe walkthrough is at file {position}: {path} ({why})\nAlready shown to the user in this file: {covered}.\n\nProduce the next walkthrough element for this file: a selection of consecutive lines and an explanation of them. Read the file first if you haven't. The tool highlights the selection and shows your explanation right below it, and the user can ask questions before moving on.\n\nGuidelines:\n- Generally move through the file from top to bottom.\n- Select at most about 15 lines, since the selection and the chat box have to fit on one screen together. Split larger pieces of code into several elements.\n- You don't need to cover every line, unless the user asked you to. For example, for a block of 200 constant definitions, select the first couple and explain that 198 more follow; only pick out others if they are particularly important. Skip boilerplate.\n- Keep the explanation concise: a few sentences of markdown on what the code does and why, relating it to what the user has already seen.\n- Take into account anything the user said during the walkthrough.\n- Do NOT edit any files.\n\nRespond with ONLY a JSON object, no prose and no code fences, of the form:\n{\"start_line\": 12, \"end_line\": 20, \"comment\": \"explanation\"}\nwhere the line numbers are 1-based lines of the file on disk. If nothing more in this file is worth covering, respond with:\n{\"done\": true}";

pub const DEFAULT_ELEMENT_CHAT_META: &str = "[Meta: the following message was entered into the chat box below lines {lines} of {path} in a codebase walkthrough tool. You explained those lines as a walkthrough element. The user expects your reply to appear in this chat box, so keep it focused and concise.]\n\n{message}";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    /// Full shell command used to launch Claude Code (run via `sh -c` in the directory).
    /// `{dir}` is replaced with the shell-quoted directory path.
    pub claude_command: String,
    pub orient_prompt: String,
    pub general_chat_meta: String,
    pub plan_prompt: String,
    pub element_prompt: String,
    pub element_chat_meta: String,
    /// Ask Claude for the next element while the user looks at the current one.
    pub prefetch: bool,
    pub dark_theme: bool,
    /// Soft-wrap long lines by default (each file can override it).
    pub wrap_lines: bool,
    pub recent_dirs: Vec<PathBuf>,
    pub show_tree: bool,
    /// UI font family ("" = system default). Applied at startup.
    pub ui_font: String,
    /// Code font family ("" = system monospace). Applied live.
    pub code_font: String,
    pub code_font_size: f32,
    /// Zoom factor for the whole UI.
    pub ui_scale: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            claude_command: DEFAULT_CLAUDE_COMMAND.into(),
            orient_prompt: DEFAULT_ORIENT_PROMPT.into(),
            general_chat_meta: DEFAULT_GENERAL_CHAT_META.into(),
            plan_prompt: DEFAULT_PLAN_PROMPT.into(),
            element_prompt: DEFAULT_ELEMENT_PROMPT.into(),
            element_chat_meta: DEFAULT_ELEMENT_CHAT_META.into(),
            prefetch: true,
            dark_theme: false,
            wrap_lines: false,
            recent_dirs: Vec::new(),
            show_tree: true,
            ui_font: String::new(),
            code_font: String::new(),
            code_font_size: crate::fonts::DEFAULT_CODE_SIZE,
            ui_scale: 1.0,
        }
    }
}

fn config_dir() -> PathBuf {
    dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("wtfren")
}

impl Settings {
    pub fn path() -> PathBuf {
        config_dir().join("settings.json")
    }

    pub fn load() -> Self {
        std::fs::read_to_string(Self::path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        let path = Self::path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(&path, json).map_err(|e| format!("Could not save {}: {e}", path.display()))
    }

    pub fn add_recent_dir(&mut self, dir: PathBuf) {
        self.recent_dirs.retain(|d| d != &dir);
        self.recent_dirs.insert(0, dir);
        self.recent_dirs.truncate(10);
    }
}

/// Values substituted into prompt templates.
#[derive(Debug, Clone, Default)]
pub struct PromptVars {
    pub dir: String,
    /// Whether the directory is a git repository, in a sentence.
    pub git_info: String,
    /// How far the walkthrough has come, for the general chat.
    pub status: String,
    pub scope: String,
    pub path: String,
    /// "3 of 12"
    pub position: String,
    pub why: String,
    pub covered: String,
    pub lines: String,
}

/// Fills `{placeholders}` in a template; appends the message if `{message}` is absent.
pub fn render_template(template: &str, vars: &PromptVars, message: &str) -> String {
    let mut out = template
        .replace("{dir}", &vars.dir)
        .replace("{git_info}", &vars.git_info)
        .replace("{status}", &vars.status)
        .replace("{scope}", &vars.scope)
        .replace("{path}", &vars.path)
        .replace("{position}", &vars.position)
        .replace("{why}", &vars.why)
        .replace("{covered}", &vars.covered)
        .replace("{lines}", &vars.lines);
    if out.contains("{message}") {
        out = out.replace("{message}", message);
    } else if !message.is_empty() {
        out.push_str("\n\n");
        out.push_str(message);
    }
    out
}

pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template() {
        let vars = PromptVars { path: "src/a.rs".into(), lines: "1-3".into(), ..Default::default() };
        let s = render_template("see {path} {lines}\n\n{message}", &vars, "hi");
        assert_eq!(s, "see src/a.rs 1-3\n\nhi");
        assert_eq!(render_template("x", &vars, "hi"), "x\n\nhi");
        assert_eq!(render_template("x", &vars, ""), "x");
        assert_eq!(shell_quote("a'b"), "'a'\\''b'");
    }

    #[test]
    fn default_prompts_keep_their_json_examples() {
        let vars = PromptVars { path: "a".into(), ..Default::default() };
        assert!(render_template(DEFAULT_ELEMENT_PROMPT, &vars, "").contains("{\"done\": true}"));
        assert!(render_template(DEFAULT_PLAN_PROMPT, &vars, "").contains("{\"files\": ["));
    }
}
