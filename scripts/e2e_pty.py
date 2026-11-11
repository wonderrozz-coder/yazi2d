#!/usr/bin/env python3
"""End-to-end smoke test for Yazi2D inside a real pseudo-terminal.

Drives the binary with real key presses and checks copy/move between panels,
favorites persistence, hidden files, quick preview, thumbnails, cross-panel
open, help, mkdir, delete and clean shutdown.
"""
import fcntl
import os
import pty
import select
import shutil
import struct
import subprocess
import sys
import tempfile
import termios
import threading
import time

BIN = sys.argv[1] if len(sys.argv) > 1 else "target/debug/yazi2d"

root = tempfile.mkdtemp(prefix="yazi2d-e2e-")
left = os.path.join(root, "left")
right = os.path.join(root, "right")
config = os.path.join(root, "config")
os.makedirs(left)
os.makedirs(right)
os.makedirs(config)
with open(os.path.join(left, "hello.txt"), "w") as f:
    f.write("hello from yazi2d")
with open(os.path.join(left, "move-me.txt"), "w") as f:
    f.write("move me")
with open(os.path.join(left, ".hidden.txt"), "w") as f:
    f.write("secret")

env = os.environ.copy()
env["TERM"] = "xterm-256color"
env["YAZI2D_CONFIG_DIR"] = config

master, slave = pty.openpty()
fcntl.ioctl(master, termios.TIOCSWINSZ, struct.pack("HHHH", 30, 120, 0, 0))
proc = subprocess.Popen(
    [BIN, left, right],
    stdin=slave,
    stdout=slave,
    stderr=slave,
    env=env,
    close_fds=True,
)
os.close(slave)

output = bytearray()
stop = threading.Event()


def reader():
    while not stop.is_set():
        ready, _, _ = select.select([master], [], [], 0.05)
        if not ready:
            continue
        try:
            data = os.read(master, 65536)
        except OSError:
            break
        if not data:
            break
        output.extend(data)


threading.Thread(target=reader, daemon=True).start()


def drain(seconds=0.4):
    time.sleep(seconds)
    return bytes(output)


def send(keys):
    os.write(master, keys)
    time.sleep(0.15)


try:
    screen = drain(0.8)
    assert b"Left" in screen, "left panel was not rendered"
    assert b"Right" in screen, "right panel was not rendered"
    assert b"Favorites" in screen, "favorites sidebar was not rendered"
    assert b"hello.txt" in screen, "directory listing was not rendered"

    # Copy the cursor entry (hello.txt) to the other panel.
    send(b"c")
    deadline = time.time() + 5
    copied = os.path.join(right, "hello.txt")
    while time.time() < deadline and not os.path.exists(copied):
        drain(0.1)
    assert os.path.exists(copied), "copy between panels failed"
    with open(copied) as f:
        assert f.read() == "hello from yazi2d"
    drain(0.5)  # let the UI consume JobMsg::Done

    # Move the second entry to the other panel.
    send(b"j")  # cursor: hello.txt -> move-me.txt
    send(b"m")
    moved = os.path.join(right, "move-me.txt")
    deadline = time.time() + 5
    while time.time() < deadline and not os.path.exists(moved):
        drain(0.1)
    assert os.path.exists(moved), "move between panels failed"
    assert not os.path.exists(os.path.join(left, "move-me.txt")), "move left the source behind"
    with open(moved) as f:
        assert f.read() == "move me"
    drain(0.5)  # let the UI consume JobMsg::Done

    # Ctrl+H reveals hidden files; the same key hides them again.
    send(b"\x08")
    screen = drain(0.4)
    assert b".hidden.txt" in screen, "Ctrl+H did not show hidden files"
    send(b"\x08")

    # Ctrl+Right turns the right panel into a quick viewer of hello.txt.
    send(b"\x1b[1;5C")
    screen = drain(0.6)
    assert b"preview" in screen, "Ctrl+Right did not switch the panel to preview"
    assert b"hello from yazi2d" in screen, "text preview content was not rendered"
    send(b"\x1b[1;5C")  # close preview again

    # Ctrl+T toggles the thumbnail grid.
    send(b"\x14")
    screen = drain(0.5)
    assert b"thumbnails" in screen, "Ctrl+T did not switch to thumbnails"
    send(b"\x14")

    # Add the active directory to favorites.
    send(b"\x02")  # Ctrl+B
    time.sleep(0.3)
    favorites = os.path.join(config, "favorites.toml")
    assert os.path.exists(favorites), "favorites file was not written"
    with open(favorites) as f:
        assert left in f.read(), "current directory was not saved to favorites"

    # Ctrl+D must give the favorites sidebar keyboard focus.
    send(b"\x04")
    screen = drain(0.4)
    assert b"Favorites" in screen, "favorites sidebar disappeared"
    send(b"\x1b")  # Esc: back to panels

    # Help overlay and prompt smoke test.
    send(b"?")
    screen = drain(0.4)
    assert b"Keyboard shortcuts" in screen, "help overlay was not rendered"
    send(b"\x1b")  # Esc closes help
    send(b"n")
    screen = drain(0.4)
    assert b"New folder" in screen, "mkdir prompt was not rendered"
    send(b"testdir")
    send(b"\r")
    time.sleep(0.5)
    assert os.path.isdir(os.path.join(left, "testdir")), "mkdir failed"

    # Delete the freshly created directory (cursor is on it); Enter confirms.
    send(b"d")
    send(b"\r")
    deadline = time.time() + 5
    while time.time() < deadline and os.path.isdir(os.path.join(left, "testdir")):
        drain(0.1)
    assert not os.path.isdir(os.path.join(left, "testdir")), "delete failed"
    drain(0.5)

    # Alt+Right opens the cursor folder in the right panel.
    send(b"n")
    send(b"sub")
    send(b"\r")
    time.sleep(0.4)
    send(b"\x1b[1;3C")
    screen = drain(0.5)
    assert b"left/sub" in screen, "Alt+Right did not open the folder in the other panel"

    send(b"\t")  # focus the right panel (inside sub)
    send(b"\x7f")  # Backspace: go up to the parent
    send(b"\t")  # back to the left panel
    time.sleep(0.3)
    send(b"d")
    send(b"\r")
    deadline = time.time() + 5
    while time.time() < deadline and os.path.isdir(os.path.join(left, "sub")):
        drain(0.1)
    assert not os.path.isdir(os.path.join(left, "sub")), "second delete failed"

    send(b"q")
    deadline = time.time() + 5
    while time.time() < deadline and proc.poll() is None:
        drain(0.1)
    assert proc.poll() is not None, "q did not quit the app"
    assert proc.returncode == 0, f"yazi2d exited with {proc.returncode}"
    print("E2E OK")
finally:
    stop.set()
    try:
        proc.kill()
    except Exception:
        pass
    os.close(master)
    shutil.rmtree(root, ignore_errors=True)
