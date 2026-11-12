# Bundling Ghostty

Ghostty is the terminal Yazi2D is developed and tested in: it is the one that
renders the half-block truecolor previews and the thumbnail grid exactly as the
screenshots show. The macOS launcher already prefers it.

## Status on Windows

Ghostty currently ships official builds for **macOS** and **Linux** only.
There is no Windows build, so the Windows archives cannot bundle it yet — the
project is tracked upstream, and the Windows page of the Ghostty documentation
still lists only the macOS and Linux packages.

## How the bundling works

`packaging/windows/build-windows.sh` copies whatever it finds in
`vendor/ghostty/` into the release zip when it is invoked with
`WITH_GHOSTTY=1`:

```sh
WITH_GHOSTTY=1 ./packaging/windows/build-windows.sh
```

The resulting archive contains:

```
yazi2d.exe
ghostty/            # copied from vendor/ghostty/
```

## What to put here

When a Windows build becomes available, drop a portable copy into this folder —
`ghostty/windows/ghostty.exe` plus its supporting files — and keep the archive
size reasonable. Nothing here is committed to git: the folder is a drop point,
not a vendored dependency, and the build script only reads it at packaging time.
