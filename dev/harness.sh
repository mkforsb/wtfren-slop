#!/bin/bash
# Test harness: runs the app on a fixture directory under Xvfb with the mock Claude, so
# it can be driven with xdotool and screenshotted.
#
#   dev/harness.sh setup             create the fixture repository and a clean home
#   dev/harness.sh start [binary]    start Xvfb and the app on the walkthrough screen
#                                    (builds the debug binary unless one is given)
#   dev/harness.sh picker [binary]   the same, on the directory screen
#   dev/harness.sh shot [file] [WxH+X+Y]   screenshot (optionally cropped)
#   dev/harness.sh stop              stop the app and Xvfb
#
# Needs Xvfb, xdotool and ImageMagick (`import`). Files go to $HARNESS_DIR (default
# /tmp/walkthrough-harness): the app's log is app.log, and prompts.log has every prompt
# the mock received. MOCK_DELAY sets how long an element takes. To use another Claude
# command, set HARNESS_CLAUDE before `setup`.
set -e
ROOT=$(cd "$(dirname "$0")/.." && pwd)
SRC=$ROOT/dev/harness
H=${HARNESS_DIR:-/tmp/walkthrough-harness}
export DISPLAY=${HARNESS_DISPLAY:-:99}

# Kills the process whose pid is in file $1, if it is still running.
kill_pidfile() {
    if [ -f "$1" ]; then
        kill "$(cat "$1")" 2>/dev/null || true
        rm -f "$1"
    fi
}

setup() {
    kill_pidfile "$H/app.pid"
    rm -rf "$H"
    mkdir -p "$H/home/.config/wtfren"
    cp -r "$SRC/fixture" "$H/repo"
    printf 'not\0text' >"$H/repo/assets/logo.bin"
    : >"$H/repo/assets/empty.txt"
    cd "$H/repo"
    git init -q -b main
    git config user.email harness@example.com
    git config user.name Harness
    git add -A && git commit -qm "Initial commit"
    local command=${HARNESS_CLAUDE:-python3 $ROOT/dev/mock_claude.py \{dir\}}
    python3 -c 'import json, sys; print(json.dumps({"claude_command": sys.argv[1]}))' "$command" \
        >"$H/home/.config/wtfren/settings.json"
    echo "fixture ready in $H"
}

start() {
    local arg=$1 bin=${2:-}
    [ -d "$H/repo" ] || setup
    if ! xdotool getmouselocation >/dev/null 2>&1; then
        setsid Xvfb "$DISPLAY" -screen 0 1600x1000x24 >/dev/null 2>&1 &
        echo $! >"$H/xvfb.pid"
        sleep 2
    fi
    kill_pidfile "$H/app.pid"
    sleep 0.3
    if [ -z "$bin" ]; then
        cargo build --quiet --manifest-path "$ROOT/Cargo.toml"
        bin=$ROOT/target/debug/wtfren
    fi
    HOME=$H/home XDG_CONFIG_HOME=$H/home/.config XDG_DATA_HOME=$H/home/.local/share MOCK_LOG=$H/prompts.log \
        setsid "$bin" $arg >"$H/app.log" 2>&1 &
    echo $! >"$H/app.pid"
    sleep 3
    # Without a window manager, keyboard focus has to be set explicitly.
    xdotool windowfocus --sync "$(xdotool search --name 'Codebase Walkthrough' | head -1)"
}

case "${1:-}" in
    setup) setup ;;
    start) start "$H/repo" "${2:-}" ;;
    picker) start "" "${2:-}" ;;
    shot) import -window root ${3:+-crop "$3"} "${2:-$H/shot.png}" ;;
    stop) kill_pidfile "$H/app.pid"; kill_pidfile "$H/xvfb.pid" ;;
    *) sed -n '2,/^[^#]/{/^#/p}' "$0"; exit 1 ;;
esac
