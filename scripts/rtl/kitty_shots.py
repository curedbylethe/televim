#!/usr/bin/env python3
"""Drive the televim binary in kitty and save right-to-left screenshots.

macOS only. Needs kitty.app, a built `target/debug/televim`, and Screen
Recording permission for the terminal that runs this script.

Each scene launches kitty with remote control, seeds an offline cache, types
keys through `kitty @`, then writes two files to `target/rtl-shots/`:

- `<scene>.png`: the window as the compositor drew it (shaping included)
- `<scene>.txt`: the cells kitty holds, as text (no shaping, no colours)

Offline means no credentials, so only the chat list and the input bar are
reachable. Conversation messages need a signed-in account and are not covered.
"""

import json
import pathlib
import shutil
import subprocess
import sys
import tempfile
import time

KITTY = "/Applications/kitty.app/Contents/MacOS/kitty"
HERE = pathlib.Path(__file__).resolve().parent
ROOT = HERE.parents[1]
BIN = ROOT / "target/debug/televim"
OUT = ROOT / "target/rtl-shots"

FONT_CONF = """\
font_family Menlo
font_size 14
window_padding_width 0
hide_window_decorations yes
remember_window_size no
initial_window_width 80c
initial_window_height 24c
"""

# Each scene: chats to seed, then steps of (keys, predicate on the screen text)
# that must hold before the next step, and the text that must be on screen
# before the screenshot is taken. Offline, the sign-in card swallows `i` until
# a command line has been opened and closed (see tests/tui_e2e.rs).
SCENES = {
    "chat_list_arabic": {
        "chats": [(1, "أمیر"), (2, "Grace")],
        "steps": [],
        "wait": "Chats (2)",
    },
    # Reference: the same logical string printed by the shell in the same kitty.
    # If it shows the same gap, the gap is the terminal's, not televim's.
    "reference_shell": {
        "argv": ["sh", "-c", "printf '%s\\n' 'سلام'; sleep 600"],
        "chats": [],
        "steps": [],
        "wait": "سلام",
    },
    # Caret at the start of the draft, before the first letter, via four Lefts.
    "draft_caret_start": {
        "chats": [(1, "Ada")],
        "steps": [
            (":", lambda s: ":" in s),
            ("\x1b", lambda s: "Sign in to Telegram" not in s),
            ("i", lambda s: "INSERT" in s),
            ("سلام", lambda s: "سلام" in s),
            ("\x1b[D" * 4, lambda s: "INSERT" in s),
        ],
        "wait": "سلام",
    },
    # Two right-to-left words, the case a reader reports as backwards.
    "draft_two_words": {
        "chats": [(1, "Ada")],
        "steps": [
            (":", lambda s: ":" in s),
            ("\x1b", lambda s: "Sign in to Telegram" not in s),
            ("i", lambda s: "INSERT" in s),
            ("سلام خوبی", lambda s: "·" in s),
        ],
        "wait": "·",
    },
    "draft_arabic": {
        "chats": [(1, "Ada")],
        "steps": [
            (":", lambda s: ":" in s),
            ("\x1b", lambda s: "Sign in to Telegram" not in s),
            ("i", lambda s: "INSERT" in s),
            ("سلام", lambda s: "سلام" in s),
        ],
        "wait": "سلام",
    },
}


def kitty(sock, *args, check=True):
    """Run a `kitty @` command against this scene's socket."""
    result = subprocess.run(
        [KITTY, "@", "--to", f"unix:{sock}", *args],
        capture_output=True,
        text=True,
    )
    if check and result.returncode != 0:
        raise RuntimeError(f"kitty @ {' '.join(args)} failed: {result.stderr}")
    return result.stdout


def screen_text(sock, check=True):
    return kitty(sock, "get-text", "--extent", "screen", check=check)


def wait_until(sock, holds, what, timeout=10.0):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if sock.exists() and holds(screen_text(sock, check=False)):
            return
        time.sleep(0.2)
    raise RuntimeError(f"{what} never held:\n{screen_text(sock, check=False)}")


def contains(text):
    return lambda screen: text in screen


def window_id(sock):
    """The CGWindowID kitty reports for its own window, for screencapture."""
    tree = json.loads(kitty(sock, "ls"))
    return tree[0]["platform_window_id"]


def shot(sock, path):
    """Crop a full-screen capture to the window: `screencapture -l` is black here."""
    bounds = subprocess.run(
        ["swift", str(HERE / "window_bounds.swift"), str(window_id(sock))],
        capture_output=True,
        text=True,
    )
    if bounds.returncode != 0:
        raise RuntimeError(f"no on-screen window for kitty: {bounds.stderr.strip()}")
    result = subprocess.run(
        ["screencapture", "-x", "-R", bounds.stdout.strip().replace(" ", ","), str(path)],
        capture_output=True,
        text=True,
    )
    if result.returncode != 0 or not path.exists():
        raise RuntimeError(
            "screencapture failed. Grant Screen Recording to the terminal "
            "running this script (System Settings > Privacy & Security > "
            f"Screen & System Audio Recording). {result.stderr.strip()}"
        )


def seed(sandbox, chats):
    """The warm-start cache `runtime.rs` reads: see `tests/common/mod.rs`."""
    file = {
        "account": None,
        "peers": {},
        "chats": [{"id": i, "title": t} for i, t in chats],
    }
    (sandbox / "televim.history.json").write_text(json.dumps(file, ensure_ascii=False))
    (sandbox / "televim.toml").write_text("")


def run_scene(name, scene):
    sandbox = pathlib.Path(tempfile.mkdtemp(prefix="televim-rtl-"))
    try:
        seed(sandbox, scene["chats"])
        (sandbox / "kitty.conf").write_text(FONT_CONF)
        # Unix socket paths are capped near 104 bytes, so the socket lives in
        # the system temp dir, not in the sandbox.
        sock = pathlib.Path(tempfile.gettempdir()) / f"tvk-{name}.sock"
        sock.unlink(missing_ok=True)
        proc = subprocess.Popen(
            [
                KITTY,
                "--config", str(sandbox / "kitty.conf"),
                "-o", "allow_remote_control=yes",
                "--listen-on", f"unix:{sock}",
                "--title", f"televim-rtl-{name}",
                "-d", str(sandbox),
                *(scene.get("argv") or [
                    "env",
                    f"TELEVIM_SESSION_PATH={sandbox / 'session.bin'}",
                    str(BIN),
                    "--config", str(sandbox / "televim.toml"),
                ]),
            ]
        )
        try:
            if "argv" not in scene:
                wait_until(sock, contains("NORMAL"), "NORMAL mode")
            for keys, holds in scene["steps"]:
                kitty(sock, "send-text", keys)
                wait_until(sock, holds, repr(keys))
            wait_until(sock, contains(scene["wait"]), repr(scene["wait"]))
            OUT.mkdir(parents=True, exist_ok=True)
            shot(sock, OUT / f"{name}.png")
            (OUT / f"{name}.txt").write_text(screen_text(sock))
            print(f"wrote {name}.png and {name}.txt")
        finally:
            proc.terminate()
            proc.wait(timeout=5)
            sock.unlink(missing_ok=True)
    finally:
        shutil.rmtree(sandbox, ignore_errors=True)


def main(names):
    for name in names or SCENES:
        run_scene(name, SCENES[name])


if __name__ == "__main__":
    try:
        main(sys.argv[1:])
    except RuntimeError as err:
        sys.exit(f"error: {err}")
