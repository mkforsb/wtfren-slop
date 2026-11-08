#!/usr/bin/env python3
"""Stream-json stand-in for `claude`, for UI work that shouldn't spend tokens.

Usage: mock_claude.py <dir>   (reads JSON lines on stdin, writes JSON lines on stdout)

It recognizes the default meta prompts:
- orientation       -> a few tool calls and a summary
- file order        -> a JSON plan of the files in <dir>
- next element      -> JSON elements of 6 lines each, 3 per file, then {"done": true}
                       (a file containing "EVERY" gets elements until its end)

In chat messages:
- "walk me through" -> an acknowledgement with the start directive
- "PERM"            -> sends a can_use_tool control_request first
- "CRASH"           -> exits with an error
- "LONG"            -> streams a long markdown answer
- "BADJSON"         -> makes the next element answer unparseable
- "ALWAYSBAD"       -> makes all element answers unparseable from now on
- anything else     -> an echo in markdown

MOCK_DELAY sets how long an element takes in seconds (default 0.3). MOCK_LONG_COMMENT
makes the explanations taller than their chat log. MOCK_LOG names a
file that receives every prompt, separated by lines of "=====". Interrupts are honored.
"""
import json
import os
import queue
import re
import sys
import threading
import time

root = sys.argv[1] if len(sys.argv) > 1 else "."
delay = float(os.environ.get("MOCK_DELAY", "0.3"))
log_path = os.environ.get("MOCK_LOG")
inbox = queue.Queue()
interrupted = threading.Event()
write_lock = threading.Lock()
replies = queue.Queue()
bad_json = False


def out(o):
    with write_lock:
        sys.stdout.write(json.dumps(o) + "\n")
        sys.stdout.flush()


def read_stdin():
    for line in sys.stdin:
        msg = json.loads(line)
        kind = msg.get("type")
        if kind == "control_request" and msg["request"]["subtype"] == "interrupt":
            interrupted.set()
            out({"type": "control_response", "response": {"subtype": "success", "request_id": msg["request_id"]}})
        elif kind == "control_response":
            replies.put(msg)
        elif kind == "user":
            inbox.put(msg["message"]["content"])
    inbox.put(None)


def pause(seconds):
    """Sleeps unless interrupted; returns whether it was."""
    return interrupted.wait(seconds)


def tool(name, **tool_input):
    out({"type": "assistant", "parent_tool_use_id": None, "message": {"id": "tool", "content": [{"type": "tool_use", "id": "t", "name": name, "input": tool_input}]}})


def answer(n, text, stream=True):
    """Sends `text` as the final message; returns False if interrupted on the way."""
    mid = f"msg-{n}"
    out({"type": "stream_event", "parent_tool_use_id": None, "event": {"type": "message_start", "message": {"id": mid}}})
    if stream:
        for chunk in [text[i:i + 12] for i in range(0, len(text), 12)]:
            out({"type": "stream_event", "parent_tool_use_id": None, "event": {"type": "content_block_delta", "delta": {"type": "text_delta", "text": chunk}}})
            if pause(0.01):
                return False
    out({"type": "assistant", "parent_tool_use_id": None, "message": {"id": mid, "content": [{"type": "text", "text": text}]}})
    out({"type": "result", "subtype": "success", "is_error": False, "result": text, "total_cost_usd": 0.001 * n})
    return True


def source_files():
    found = []
    for base, dirs, names in os.walk(root):
        dirs[:] = [d for d in dirs if d != ".git" and d != "target"]
        for name in names:
            found.append(os.path.relpath(os.path.join(base, name), root))
    return sorted(found, key=lambda p: (not p.endswith(".md"), p))


def plan():
    return json.dumps({"files": [{"path": p, "why": f"Mock reason for {p}."} for p in source_files()]})


def element(text):
    global bad_json
    if bad_json:
        bad_json = bad_json == "always" and "always"
        return "Sorry, I can't produce JSON today."
    path = re.search(r"file \d+ of \d+: (\S+)", text).group(1)
    covered = re.search(r"Already shown to the user in this file: (.*)\.", text).group(1)
    shown = [int(x) for x in re.findall(r"\d+", covered)]
    full = os.path.join(root, path)
    lines = open(full, errors="replace").read().count("\n") or 1
    start = (max(shown) if shown else 0) + 1
    every = "EVERY" in open(full, errors="replace").read()
    if start > lines or (len(covered.split(",")) >= 3 and not every):
        return json.dumps({"done": True})
    end = min(start + 5, lines)
    comment = f"**Lines {start}-{end}** of `{path}`: a mock explanation of what happens here, and why it matters.\n\n- one point\n- another point"
    if os.environ.get("MOCK_LONG_COMMENT"):
        comment += "".join(f"\n- padding point {i}" for i in range(30))
    return json.dumps({"start_line": start, "end_line": end, "comment": comment})


def main():
    global bad_json
    threading.Thread(target=read_stdin, daemon=True).start()
    out({"type": "system", "subtype": "init", "session_id": "mock-session", "model": "mock-model", "permissionMode": "default", "cwd": root})
    n = 0
    while (text := inbox.get()) is not None:
        n += 1
        interrupted.clear()
        if log_path:
            with open(log_path, "a") as f:
                f.write(text + "\n=====\n")
        # The user's words follow the closing bracket of the meta text.
        user = text.rsplit("]\n\n", 1)[-1]
        finished = True
        if "Get somewhat briefly oriented" in text:
            for name, arg in [("Bash", {"command": "ls -la"}), ("Read", {"file_path": "README.md"}), ("Bash", {"command": "git log --oneline -20"})]:
                tool(name, **arg)
                finished = not pause(0.3)
                if not finished:
                    break
            if finished:
                finished = answer(n, "This is a **small mock project** with a lexer and a parser.\n\nWould you like to keep chatting about it in general, or jump straight into a file-by-file walkthrough, of the full codebase or of specific parts of it?")
        elif "Decide which files to go through" in text:
            tool("Bash", command="git ls-files")
            finished = not pause(0.5)
            if finished:
                out({"type": "assistant", "parent_tool_use_id": None, "message": {"id": "note", "content": [{"type": "text", "text": "Let me check how the modules depend on each other."}]}})
                finished = answer(n, plan(), stream=True)
        elif "Produce the next walkthrough element" in text:
            tool("Read", file_path=re.search(r"file \d+ of \d+: (\S+)", text).group(1))
            finished = not pause(delay)
            if finished:
                finished = answer(n, element(text))
        elif "CRASH" in user:
            sys.stderr.write("mock: simulated crash\n")
            sys.exit(3)
        elif "PERM" in user:
            out({"type": "control_request", "request_id": f"req-{n}", "request": {"subtype": "can_use_tool", "tool_name": "Bash", "input": {"command": "rm -rf /tmp/nothing", "description": "Mock dangerous command"}, "description": "Mock dangerous command", "permission_suggestions": [{"type": "addRules"}], "decision_reason": "mock"}})
            decision = replies.get()["response"]["response"]["behavior"]
            finished = answer(n, f"Permission decision received: **{decision}**.")
        elif "LONG" in user:
            finished = answer(n, f"Long echo #{n}.\n\n" + "\n".join(f"{i}. Point number {i}, padded out so that the reply wraps and takes real vertical space in the chat log." for i in range(1, 41)))
        elif "ALWAYSBAD" in user:
            bad_json = "always"
            finished = answer(n, "All elements will be broken from now on.")
        elif "BADJSON" in user:
            bad_json = True
            finished = answer(n, "The next element will be broken.")
        elif "walk me through" in user.lower() and "chat box below lines" not in text:
            scope = re.sub(r"(?i).*walk me through\s*", "", user).strip() or "full"
            finished = answer(n, f"Sure, let's go!\n\n[[START_WALKTHROUGH: {scope}]]")
        else:
            tool("Read", file_path="README.md")
            finished = answer(n, f"Echo #{n}: you said *{user}*.\n\n```rust\nfn main() {{}}\n```")
        if not finished:
            out({"type": "result", "subtype": "error_during_execution", "is_error": True, "result": "", "total_cost_usd": 0.001 * n})


main()
