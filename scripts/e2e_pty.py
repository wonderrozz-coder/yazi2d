#!/usr/bin/env python3
"""End-to-end smoke test for Yazi2D inside a real pseudo-terminal.

Drives the binary with real key presses and checks copy/move between panels,
favorites persistence, hidden files, quick preview, thumbnails, cross-panel
open, help, mkdir, delete and clean shutdown.
"""
import codecs
import fcntl
import os
import pty
import re
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

# Every run gets its own trash, so the delete step can assert where the entry went
# without touching the developer's real one.
TRASH = tempfile.mkdtemp(prefix="yazi2d-trash-")
os.environ["YAZI2D_TRASH_DIR"] = TRASH

WIDTH = 120
HEIGHT = 30

# Terminal escape sequences split words in the raw stream, so text assertions
# run against a reconstructed screen instead.
ANSI = re.compile(rb"\x1b\[[0-9;?]*[A-Za-z]")


class Screen:
    """A minimal terminal screen, so assertions see what a human sees.

    Checking the raw byte stream is unreliable: ratatui repaints only the cells
    that changed, so a line like `hello from yazi2d` never arrives as one
    contiguous string — the cursor jumps between runs and unchanged spaces are
    skipped. This class applies absolute cursor moves, erases and printable
    text, and then hands back the visible rows.
    """

    def __init__(self, width, height):
        self.width = width
        self.height = height
        self.decoder = codecs.getincrementaldecoder("utf-8")("replace")
        self.cells = [[" "] * width for _ in range(height)]
        self.row = 0
        self.col = 0
        self.state = "text"   # text | esc | csi | osc
        self.pending = ""

    def feed(self, data):
        for char in self.decoder.decode(data):
            self._char(char)

    def _char(self, char):
        if self.state == "text":
            if char == "\x1b":
                self.state = "esc"
            elif char == "\r":
                self.col = 0
            elif char == "\n":
                self.row = min(self.row + 1, self.height - 1)
            elif char == "\b":
                self.col = max(self.col - 1, 0)
            elif char >= " ":
                self._put(char)
        elif self.state == "esc":
            if char == "[":
                self.state = "csi"
                self.pending = ""
            elif char == "]":
                self.state = "osc"
                self.pending = ""
            elif char in "()#":
                # Charset selection: the next character is a designator.
                self.state = "text"
            else:
                self.state = "text"
        elif self.state == "csi":
            if "@" <= char <= "~":
                self._csi(self.pending, char)
                self.state = "text"
            else:
                self.pending += char
        elif self.state == "osc":
            if char == "\x07":
                self.state = "text"
            elif char == "\x1b":
                self.state = "esc"
            elif char == "?" and not self.pending:
                # OSC 52 copy payloads can be long; keep nothing.
                self.pending = "?"

    def _csi(self, params, final):
        # Private modes (`?1049h`), SGR (`m`), scroll regions (`r`) and the
        # like need no screen update here.
        if params.startswith("?"):
            if final in "hl":
                return
        numbers = [int(chunk) for chunk in params.split(";") if chunk.isdigit()]

        def arg(index, default):
            return numbers[index] if len(numbers) > index else default

        if final in "Hf":
            self.row = max(0, min(arg(0, 1) - 1, self.height - 1))
            self.col = max(0, min(arg(1, 1) - 1, self.width - 1))
        elif final == "d":
            self.row = max(0, min(arg(0, 1) - 1, self.height - 1))
        elif final == "G":
            self.col = max(0, min(arg(0, 1) - 1, self.width - 1))
        elif final == "A":
            self.row = max(0, self.row - arg(0, 1))
        elif final == "B":
            self.row = min(self.height - 1, self.row + arg(0, 1))
        elif final == "C":
            self.col = min(self.width - 1, self.col + arg(0, 1))
        elif final == "D":
            self.col = max(0, self.col - arg(0, 1))
        elif final == "J":
            mode = arg(0, 0)
            if mode == 2 or mode == 3:
                self.cells = [[" "] * self.width for _ in range(self.height)]
            elif mode == 0:
                self._clear_to_end()
            elif mode == 1:
                for r in range(self.row):
                    self.cells[r] = [" "] * self.width
                for c in range(self.col):
                    self.cells[self.row][c] = " "
        elif final == "K":
            mode = arg(0, 0)
            if mode == 0:
                for c in range(self.col, self.width):
                    self.cells[self.row][c] = " "
            elif mode == 2:
                self.cells[self.row] = [" "] * self.width

    def _clear_to_end(self):
        for c in range(self.col, self.width):
            self.cells[self.row][c] = " "
        for r in range(self.row + 1, self.height):
            self.cells[r] = [" "] * self.width

    def _put(self, char):
        if 0 <= self.row < self.height and 0 <= self.col < self.width:
            self.cells[self.row][self.col] = char
        self.col = min(self.col + 1, self.width - 1)

    def text(self):
        return "\n".join("".join(row).rstrip() for row in self.cells)


def compact(data):
    """Letters/digits only, so ratatui's cell diffs cannot split a word."""
    if isinstance(data, bytes):
        text = data.decode("utf-8", "ignore")
    else:
        text = data
    return "".join(char for char in text if char.isalnum())


def has_ordered(data, needle):
    """True when `needle` appears in order, ignoring inserted redraw noise."""
    haystack = compact(data).lower()
    needle = "".join(char for char in needle.lower() if char.isalnum())
    position = 0
    for char in haystack:
        if position < len(needle) and char == needle[position]:
            position += 1
    return position == len(needle)

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
# The screen is fed from the reader thread, so it is always up to date and
# assertions look at what a human would see, not at raw escape bytes.
view = Screen(WIDTH, HEIGHT)
screen_lock = threading.Lock()


def reader():
    while not stop.is_set():
        try:
            ready, _, _ = select.select([master], [], [], 0.05)
        except (OSError, ValueError):
            # The test closes the master fd while this thread may still be in
            # select(); that is a normal shutdown, not a failure.
            return
        if not ready:
            continue
        try:
            data = os.read(master, 65536)
        except OSError:
            return
        if not data:
            return
        output.extend(data)
        with screen_lock:
            view.feed(data)


threading.Thread(target=reader, daemon=True).start()


def drain(seconds=0.4):
    """Wait, then return what the terminal screen shows.

    Returned as bytes so existing `b"..." in screen` assertions keep working;
    the content is the reconstructed screen, not the raw stream.
    """
    time.sleep(seconds)
    with screen_lock:
        return view.text().encode("utf-8", "replace")


def raw():
    """The raw byte stream, for assertions about a specific escape payload."""
    return bytes(output)


def send(keys):
    os.write(master, keys)
    time.sleep(0.15)


def until(needle, timeout=5.0, interval=0.05):
    """Poll the screen until `needle` (str or bytes) appears; return the screen.

    Fixed sleeps race against asynchronous work — the preview loader, the
    directory listing and the image decoder all finish on a worker thread — so
    every assertion that depends on such a result waits for it explicitly.
    """
    if isinstance(needle, bytes):
        needle_bytes = needle
    else:
        needle_bytes = needle.encode("utf-8")
    deadline = time.time() + timeout
    while True:
        with screen_lock:
            text = view.text().encode("utf-8", "replace")
        if needle_bytes in text or time.time() >= deadline:
            return text
        time.sleep(interval)


def row_of(needle, timeout=5.0):
    """Screen row (1-based, as the mouse protocol reports it) containing `needle`.

    Clicking a hard-coded row is fragile: the row depends on how many entries a
    listing has, so the test looks the row up on the screen it actually sees.
    Returns `None` when the text never appears.
    """
    text = needle if isinstance(needle, str) else needle.decode("utf-8", "replace")
    deadline = time.time() + timeout
    while True:
        with screen_lock:
            lines = view.text().split("\n")
        for index, line in enumerate(lines):
            if text in line:
                return index + 1
        if time.time() >= deadline:
            return None
        time.sleep(0.05)


def double_click(row, column=30):
    """Two left clicks on one row, `row` being 1-based as the terminal reports it."""
    click = f"\x1b[<0;{column};{row}M\x1b[<0;{column};{row}m".encode()
    send(click)
    send(click)


try:
    screen = until(b"hello.txt", timeout=8)
    assert b"Left" in screen, "left panel was not rendered"
    assert b"Right" in screen, "right panel was not rendered"
    assert b"Favorites" in screen, "favorites sidebar was not rendered"
    assert b"hello.txt" in screen, "directory listing was not rendered"

    # Copy the cursor entry (hello.txt) to the other panel. Copy now asks for
    # confirmation, so Enter follows the key. The key is silent while a panel is
    # still loading (that is deliberate: file operations refuse to start against
    # a half-loaded listing), so retry like a user would.
    def press_for(key, needle, tries=20, timeout=2.0):
        """Press `key` until `needle` shows up, then return that screen."""
        for _ in range(tries):
            send(key)
            text = until(needle, timeout=timeout)
            if needle.encode("utf-8") in text:
                return text
        return until(needle, timeout=timeout)

    screen = press_for(b"c", "Confirm copy")
    assert b"Confirm copy" in screen, "copy confirmation was not shown"
    send(b"\r")
    deadline = time.time() + 5
    copied = os.path.join(right, "hello.txt")
    while time.time() < deadline and not os.path.exists(copied):
        drain(0.1)
    assert os.path.exists(copied), "copy between panels failed"
    with open(copied) as f:
        assert f.read() == "hello from yazi2d"
    drain(0.5)  # let the UI consume JobMsg::Done

    # Move the second entry to the other panel.
    # A real terminal with keyboard enhancement reports the key release, and the
    # app needs it to stop its own acceleration. Sending only the press used to
    # leave `j` repeating for seconds, which wrapped the cursor onto `..` and
    # made the next file operation report "Nothing selected" — the exact bug the
    # release event prevents, so the test sends one like a terminal would.
    send(b"j")
    send(b"\x1b[106;1:3u")  # key release for 'j' (kitty keyboard protocol)
    screen = press_for(b"m", "Confirm move")
    assert b"Confirm move" in screen, "move confirmation was not shown"
    send(b"\r")
    moved = os.path.join(right, "move-me.txt")
    deadline = time.time() + 5
    while time.time() < deadline and not os.path.exists(moved):
        drain(0.1)
    assert os.path.exists(moved), "move between panels failed"
    assert not os.path.exists(os.path.join(left, "move-me.txt")), "move left the source behind"
    with open(moved) as f:
        assert f.read() == "move me"
    drain(0.5)  # let the UI consume JobMsg::Done

    # Escape cancels a pending copy. The prompt has to be on screen before Esc
    # is sent, otherwise Esc arrives while nothing is pending and is simply
    # ignored — which made this check fail now and then.
    screen = press_for(b"c", "Confirm copy")
    assert b"Confirm copy" in screen, "copy confirmation was not shown"
    send(b"\x1b")
    screen = until(b"cancelled")
    assert b"cancelled" in screen.lower(), "copy was not cancelled"

    # Ctrl+H reveals hidden files; the same key hides them again.
    send(b"\x08")
    screen = until(b".hidden.txt")
    assert b"hidden" in screen, "Ctrl+H did not show hidden files"
    send(b"\x08")

    # Ctrl+P turns the right panel into a quick viewer of hello.txt.
    send(b"\x10")
    screen = until(b"preview")
    assert b"preview" in screen, "Ctrl+P did not switch the panel to preview"
    # The preview is read on a worker thread, so wait for the content itself.
    screen = until(b"hello from yazi2d")
    assert b"hello from yazi2d" in screen, "text preview content was not rendered"
    send(b"\x10")  # close preview again

    # Ctrl+T toggles the thumbnail grid.
    send(b"\x14")
    screen = until(b"thumbnails")
    assert b"thumbnails" in screen, "Ctrl+T did not switch to thumbnails"
    send(b"\x14")

    # Add the active directory to favorites.
    send(b"\x02")  # Ctrl+B
    favorites = os.path.join(config, "favorites.toml")
    deadline = time.time() + 5
    while time.time() < deadline and not os.path.exists(favorites):
        drain(0.1)
    assert os.path.exists(favorites), "favorites file was not written"
    with open(favorites) as f:
        assert left in f.read(), "current directory was not saved to favorites"

    # Ctrl+A selects everything; Esc clears it again.
    send(b"\x01")
    screen = until(b"item(s)")
    assert has_ordered(screen, "item(s)"), "Ctrl+A did not select entries"
    send(b"\x1b")

    # Sort menu: choose size, then leave the menu.
    send(b"s")
    screen = until(b"SORT ORDER")
    assert has_ordered(screen, "SORT ORDER"), "sort menu was not rendered"
    send(b"s")
    screen = until(b"Sorted by size")
    assert has_ordered(screen, "Sorted by size"), "size sort was not applied"

    # Colour menu.
    send(b"\x0b")  # Ctrl+K
    screen = until(b"Colours")
    assert has_ordered(screen, "Colours"), "colour menu was not rendered"
    send(b"\x1b")

    # Shift+Tab must give the favorites sidebar keyboard focus.
    send(b"\x1b[Z")
    screen = until(b"Enter go")
    assert b"Favorites" in screen, "favorites sidebar disappeared"
    assert b"Enter go" in screen, "Shift+Tab did not focus the sidebar"
    send(b"\x1b[Z")  # back to panels

    # Ctrl+D does the same: it is advertised in the README, --help and the
    # in-app help, and once only the docs had it.
    send(b"\x04")
    screen = until(b"Enter go")
    assert b"Enter go" in screen, "Ctrl+D did not focus the sidebar"

    # Reordering a bookmark is a rebindable action now, so it also has a plain
    # letter for Macs (Fn+↑/↓ is not usable in most terminals).
    assert has_ordered(screen, "PgUp/PgDn move row"), "the sidebar reorder hint is missing"
    assert has_ordered(screen, "S separator"), "the separator hint is missing"

    # Back to the panels: the rest of the script is panel work.
    send(b"\x04")
    screen = until(b"Left:")

    # Help overlay and prompt smoke test.
    send(b"?")
    screen = until(b"Keyboard shortcuts")
    assert b"Keyboard shortcuts" in screen, "help overlay was not rendered"
    send(b"\x1b")  # Esc closes help
    send(b"n")
    screen = until(b"New folder")
    assert b"New folder" in screen, "mkdir prompt was not rendered"
    send(b"testdir")
    send(b"\r")
    deadline = time.time() + 5
    while time.time() < deadline and not os.path.isdir(os.path.join(left, "testdir")):
        drain(0.1)
    assert os.path.isdir(os.path.join(left, "testdir")), "mkdir failed"

    # Delete the freshly created directory (cursor is on it); Enter confirms.
    # Wait for the confirmation first: Enter sent before the prompt exists is
    # swallowed by the panel instead of confirming.
    send(b"d")
    screen = until(b"to the trash")
    assert b"to the trash" in screen, "delete confirmation was not shown"
    send(b"\r")
    deadline = time.time() + 5
    while time.time() < deadline and os.path.isdir(os.path.join(left, "testdir")):
        drain(0.1)
    assert not os.path.isdir(os.path.join(left, "testdir")), "delete failed"
    # The delete must go to the trash, not straight to /dev/null: that is the
    # whole point of the safety change, and it is easy to regress.
    trash_files = os.path.join(TRASH, "files")
    assert os.path.isdir(trash_files), "no trash directory was created"
    assert "testdir" in os.listdir(trash_files), (
        f"testdir did not reach the trash: {os.listdir(trash_files)}"
    )
    drain(0.6)

    # Ctrl+O opens the cursor folder in the right panel.
    send(b"n")
    send(b"sub")
    send(b"\r")
    deadline = time.time() + 5
    while time.time() < deadline and not os.path.isdir(os.path.join(left, "sub")):
        drain(0.1)
    send(b"\x0f")
    screen = until(b"left/sub")
    assert b"left/sub" in screen, "Ctrl+O did not open the folder in the other panel"

    send(b"\t")  # focus the right panel (inside sub)
    send(b"\x7f")  # Backspace: go up to the parent
    send(b"\t")  # back to the left panel
    screen = until(b"sub", timeout=2)
    send(b"d")
    screen = until(b"to the trash")
    assert b"to the trash" in screen, "second delete confirmation was not shown"
    send(b"\r")
    deadline = time.time() + 5
    while time.time() < deadline and os.path.isdir(os.path.join(left, "sub")):
        drain(0.1)
    assert not os.path.isdir(os.path.join(left, "sub")), "second delete failed"

    # Trackpad gestures are checked last because they can change directory.
    send(b"\x1b[<64;20;10M")  # scroll up
    send(b"\x1b[<65;20;10M")  # scroll down
    screen = drain(0.3)
    assert b"Left" in screen and b"Right" in screen, "panels broke after vertical scrolling"

    # Horizontal scroll must be inert: it used to dive into folders.
    send(b"\x1b[<67;20;10M" * 8)  # scroll right
    send(b"\x1b[<66;20;10M" * 8)  # scroll left
    screen = drain(0.4)
    assert b"Left" in screen and b"Right" in screen, "panels broke after horizontal scroll"

    # A single tap must not open anything; a double tap opens the entry. The
    # row is looked up on the screen, because a hard-coded one silently missed
    # and made this check vacuous.
    send(b"n")  # mkdir prompt -> create a folder to enter
    send(b"taps")
    send(b"\r")
    deadline = time.time() + 5
    while time.time() < deadline and not os.path.isdir(os.path.join(left, "taps")):
        drain(0.1)
    row = row_of(b"taps/")
    assert row is not None, "the new folder is not on the screen"
    send(f"\x1b[<0;30;{row}M\x1b[<0;30;{row}m".encode())  # single tap
    screen = drain(0.4)
    assert b"left/taps" not in screen, "a single tap opened the folder"
    double_click(row)
    # A double tap enters the folder, so the path bar shows it; wait for that
    # instead of sleeping and hoping.
    screen = until(b"left/taps", timeout=3)
    assert b"left/taps" in screen, "double tap did not open the folder"

    # Stepping past the last bookmark enters the Disks section: the cursor used
    # to stop there, so the volumes could not be reached from the keyboard at
    # all. Browsing a volume also reveals it in the active panel, which changes
    # the folder — that is why this check runs last.
    send(b"\x04")
    screen = until(b"Enter go")
    for _ in range(12):
        send(b"j")
    screen = drain(0.4)
    assert b"Disks" in screen, "the Disks section vanished"
    send(b"\x04")

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
