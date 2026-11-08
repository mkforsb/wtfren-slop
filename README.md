# wtfren

A Rust/Iced desktop app where Claude Code walks you through a codebase, file by file.
It is derived from [reviewfren](https://github.com/mkforsb/reviewfren-slop).

```
cargo run --release                # start on the directory picker
cargo run --release -- /path/dir   # jump straight to the walkthrough of a directory
```

Requirements: `claude`. `git` is optional. Any directory works, but in a git repository
Claude can also learn from the history.

## Workflow

1. **Directory screen**: "Choose directory…" opens the system folder dialog
   (xdg-desktop-portal). You can also type or paste a path, or pick a recently used one.
2. **Orientation**: `claude` starts in the background and gets a silent prompt to orient
   itself. Its tool calls and activity show in the chat panel on the right, followed by a
   brief summary and the question of how to go on.
3. **General chat**: ask anything about the codebase. To start the walkthrough, use
   **▶ Full walkthrough**, **▶ Part of it…**, or simply say so in the chat ("let's go file
   by file, but only the lexer"). In that last case, Claude's reply ends with a hidden
   `[[START_WALKTHROUGH: <scope>]]` line.
4. **File order**: Claude is silently asked for a plan, which is shown in the chat panel.
   Paths that aren't files inside the directory are left out.
5. **Files** (loop a): the next file is added to the page and the file tree, and the page
   scrolls to its top. Files that can't be shown (binary, empty, over 2 MiB) are skipped,
   with a note in the chat panel.
6. **Elements** (loop b): Claude is silently asked for the next element of the file, a few
   lines and an explanation. The lines get highlighted, and a chat box with the explanation
   opens under them. Its input is focused and placed at the very bottom of the view, with
   the selection above it. Near the top of a file the view stays at the file's header
   rather than showing the end of the previous file. Ask follow-up questions, or press
   **👍** for the next element. When Claude says the file is done, the next file follows.

Past elements remain as a `✦ n` badge at the end of their selection, with a bar in front
of their lines. Click a badge to reopen the explanation and go on chatting there.

The page also has what `reviewfren` has: collapsible files, a per-file **↩ Wrap**
toggle, and code text selection (drag, double-click a word, triple-click a line,
Shift+click to extend, Ctrl+C to copy, Esc to clear). The file tree has a text filter and
extension filters, and clicking a file scrolls to it. The entry of the file you appear to
be looking at is bold: the section under the mouse, or else the first section whose header
is visible, or else the section filling the view. The chat panel can be hidden, but is
always there when a walkthrough screen opens.

## Claude

The `claude` process starts when the walkthrough screen opens: `sh -c '<command>'` in the
directory, talking stream-json over stdin/stdout. It is killed (as a process group) when
you leave the screen. All chats and silent prompts share one session. Requests are queued,
and each `result` is routed back to whatever asked for it.

* **Silent prompts** (orientation, file order, next element) are never shown, but their
  tool calls, activity and any remarks Claude makes on the way appear in the chat panel.
  **■ Stop** interrupts them. The file order and element answers are JSON, which may
  come wrapped in prose or a code fence.
* **Prefetching**: while you look at an element, Claude already works on the next one, so
  👍 usually moves on at once. When Claude says the file is done, it continues with the
  first element of the next file. Sending a message drops what was prefetched and interrupts
  a prefetch underway, so that your question is answered first and what comes next can take
  it into account; prefetching resumes after the answer. A failed prefetch is not repeated
  until you press 👍. It can be turned off in the settings.
* **Failures**: if the element you are waiting for doesn't arrive, a banner on the file
  says why and offers **↻ Retry**. Element prompts say what has been covered of the file, so
  a walkthrough also survives **Restart** in the status bar, which starts a new session.
* **Permission prompts** (`control_request` / `can_use_tool`) open a modal showing the
  tool, its description and input. **Allow**, **Always allow** (uses Claude's
  `permission_suggestions`) and **Deny** send the matching `control_response`.

### Settings (⚙, saved to `~/.config/wtfren/settings.json`)

* **Claude command**: the full shell command. `{dir}` expands to the shell-quoted directory
  path. The default takes away the editing tools, since a walkthrough shouldn't change the
  code (and line numbers have to stay valid), and allows read-only `git` commands and `ls`
  without a prompt. For Docker, use `-i`, **not** `-it`, since stdin is a pipe:
  `docker exec -i -w {dir} dockerclaude claude -p --input-format stream-json …`
  (the directory has to be mounted at the same path inside the container).
  Changing the command restarts Claude.
* **Meta prompts** for orientation, general chat, file order, next element and element
  chat. The settings panel lists each one's placeholders and the answer format the tool
  expects.
* **Prepare the next element ahead of time**, **Wrap long lines**, and **Appearance**: dark
  theme, UI font, code font, code font size and UI zoom. iced only reads the UI font at
  startup, so changing it turns Save into **Save & restart**.

## Code map

| file | purpose |
| --- | --- |
| `src/walkthrough.rs` | UI-free core: parsing Claude's answers, and the file and element loops with prefetching |
| `src/app.rs` | state, messages, update logic, Claude job queue, scrolling |
| `src/view.rs` | all screens and overlays |
| `src/code_view.rs` | custom code widget: drawing, marks, selection, inserted chat boxes |
| `src/tracker.rs` | works out which file the user is looking at |
| `src/selectable_text.rs` | selectable rich text, used for markdown in chats |
| `src/files.rs` | checking the directory, loading files |
| `src/highlight.rs` | syntect highlighting (two-face syntaxes/themes) |
| `src/claude.rs` | `claude` subprocess and stream-json protocol |
| `src/settings.rs` | settings and default prompts |
| `src/fonts.rs` | installed font families, active UI/code fonts |
| `src/style.rs` | theme and shared styles |

## Development

```
cargo test
```

`dev/mock_claude.py` is a stream-json stand-in for `claude` that understands the default
prompts, useful for UI work that shouldn't spend tokens. Set the Claude command to
`python3 /path/to/dev/mock_claude.py {dir}`. Its docstring lists what it can simulate:
slow answers, bad JSON, crashes, permission prompts and more.

`dev/harness.sh` runs the app on a fixture repository under Xvfb, so it can be driven with
`xdotool` and screenshotted (needs Xvfb, xdotool and ImageMagick). Run it without arguments
for the details. `dev/find_thumb.py` finds the 👍 button in a screenshot (needs Pillow).
The harness uses its own `HOME`, so to try it with the real `claude`, set for example
`HARNESS_CLAUDE="env HOME=$HOME claude -p …"` before `dev/harness.sh setup`.
