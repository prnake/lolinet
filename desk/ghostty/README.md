# lolidesk-ghostty

Rust bindings to **libghostty-vt**, the terminal core of [Ghostty](https://github.com/ghostty-org/ghostty), loaded at run time. There is no link-time dependency, so `cargo check`, `clippy` and builds work without the library.

lolidesk uses it on both ends of a remote terminal:

- **Host**: one `Terminal` per session, fed by the PTY. Each attached client gets a `Mirror`. `Mirror::frame` returns VT sequences that bring the client's terminal from what it showed after the previous frame to what the host shows now: every cell, the scrollback, the cursor and the input modes. `Terminal::history` returns the scrollback and the screen down to the cursor, saved to disk and replayed above a fresh shell after a host restart.
- **Client**: its own `Terminal` eats the frames (with a no-op `Handler`, since the host answers the program's queries). `RenderState` reads it for drawing: resolved colors, graphemes in reused buffers, per-row dirty flags and row ids, cursor, scrollbar. `KeyEncoder`, `MouseEncoder`, `paste` and `focus` encode input according to the modes the frames set.

## The library

The library is built from Ghostty commit `befcdfd2c3a1cb24d9ec886e93c95b2b5daa7028` and prebuilt by [mygo](https://github.com/egoist/mygo) (MIT). [`libghostty-vt.json`](libghostty-vt.json) pins the URL and sha256 for each platform. To fetch one:

```sh
desk/scripts/fetch-ghostty.sh darwin-arm64 path/to/dir   # or darwin-amd64, linux-amd64, linux-arm64
```

The script verifies the sha256 and exits non-zero on a mismatch. If the right file is already there, it does nothing.

At run time `load()` looks for `libghostty-vt.dylib` (macOS) or `libghostty-vt.so` (Linux), in this order:

1. `$LOLIDESK_GHOSTTY_VT`
2. next to the executable
3. `<exe>/../Frameworks`
4. `<exe>/../Resources`
5. `<exe>/../lib`

It loads the first file it finds, once. It then checks every struct this crate passes, and the enum values it hardcodes, against the library's own type manifest (`ghostty_type_json`). A library of another version fails to load instead of corrupting memory.

## Tests

`cargo test -p lolidesk-ghostty` needs the library. Set `LOLIDESK_GHOSTTY_VT`, or let the tests find mygo's download cache (`~/Library/Caches/mygo/natives/<sha256>/` on macOS, `~/.cache/mygo/natives/<sha256>/` on Linux; only searched under `cfg(test)`). Without it, each test prints a skip message and passes.

The mirror tests feed a host terminal scripted and random output in many steps. After each step they feed the frame to a client terminal and compare the two cell by cell: text, style, width, link, wrap, scrollback, cursor and modes.

## Not ported

- Kitty graphics in mirrors (mygo's `mirror_images.go`). The terminal's image limit is 0, so programs fall back.
- OSC 52 clipboard forwarding. Programs' clipboard writes go nowhere; reads are refused by the library.

## Attribution

`Mirror`, `History`, the terminal callbacks and the encoders are ported from mygo's `plugins/terminal` (MIT, Copyright (c) 2026 MyGo contributors). The kero server (egoist/kero, MIT) showed how to use them. libghostty-vt is part of Ghostty (MIT).
