# desk/

lolidesk: a remote terminal and remote desktop over the lolinet overlay. A Rust workspace, built and released on its own, never linked into `lolinet`. Usage, protocol and distribution: [docs/lolidesk.md](../docs/lolidesk.md).

| Path | Builds | Contents |
| :-- | :-- | :-- |
| `proto/` | library | Wire protocol: messages (postcard), datagram fragment header, video settings, palette |
| `ghostty/` | library | libghostty-vt bindings (loaded at run time), `Mirror` and `History`, render state and input encoders. See [ghostty/README.md](ghostty/README.md) |
| `host/` | library | `lolidesk host`: QUIC server, terminal sessions (PTY + headless libghostty-vt) and their persistence, agent detection, macOS capture and encoding (`src/desktop/mac.rs`), input injection (`src/input.rs`), LaunchAgent install |
| `core/` | library | Client side: connection and fingerprint pinning, remembered hosts, VideoToolbox decoding into NV12 pixel buffers |
| `client/` | `lolidesk` | The one program: `lolidesk host …` (from `host/`) and the client side, command line and TUI |
| `app/` | `lolidesk-app` → `Lolidesk.app` | Desktop app: GPUI + gpui-component, macOS only (a stub elsewhere) |
| `scripts/` | | `fetch-ghostty.sh` (the pinned libghostty-vt for a platform), `bundle-app.sh` (Lolidesk.app, with `lolidesk` in `Contents/Helpers`: `Contents/MacOS/lolidesk` would overwrite `Lolidesk` on a case-insensitive disk) |

## Build and check

Run from the repository root:

```sh
make check-desk     # fetches libghostty-vt into desk/.ghostty/, then fmt, clippy and tests
make desk           # release builds in desk/target/release/
make desk-app       # desk/app-out/Lolidesk.app
```

Or by hand from `desk/`, which is what [desk.yml](../.github/workflows/desk.yml) runs on every pull request:

```sh
sh scripts/fetch-ghostty.sh darwin-arm64 .ghostty          # or darwin-amd64, linux-amd64, linux-arm64
export LOLIDESK_GHOSTTY_VT=$PWD/.ghostty/libghostty-vt.dylib
cargo fmt --all --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
cargo build --locked --release --workspace
sh scripts/bundle-app.sh target/release/lolidesk-app target/release/lolidesk "$LOLIDESK_GHOSTTY_VT" app-out
```

| | |
| :-- | :-- |
| **macOS** | Xcode 26 or later (macOS 26 SDK). The capture library's Metal bridge uses types from that SDK. CI sets `DEVELOPER_DIR` to Xcode 26 on the `macos-15` image. The first build of the app compiles GPUI and takes a few minutes |
| **Linux** | Terminal only: host and command line, no app. No extra system packages |
| **Targets in CI** | `aarch64-apple-darwin`, `x86_64-apple-darwin`, `x86_64-unknown-linux-gnu` |
| **libghostty-vt** | Loaded at run time from `$LOLIDESK_GHOSTTY_VT`, beside the executable, or the app's `Contents/Frameworks`. Tests that need it print a skip message without it |
| **GPUI** | `gpui-kit` pins one `gpui-pre` snapshot of Zed's GPUI. Upgrade them together; a snapshot may change GPUI's API |

## Smoke test

CI starts the host on loopback, opens a session from the TUI client with its stdin on a pipe, and runs `echo HELLO-$((6*7))`. The echo only contains `$((6*7))`, so `HELLO-42` in the output proves the command ran in the host's PTY and the frames came back. A wrong token must be refused. It then opens a session, runs `echo MARK-$((20+22))`, detaches, stops the host with SIGTERM (which saves the sessions), starts it again, and expects the session listed as restored with `MARK-42` in its first frame. To run the first part locally:

```sh
export LOLIDESK_GHOSTTY_VT=$PWD/.ghostty/libghostty-vt.dylib
TOKEN=$(./target/release/lolidesk host token | awk '/^token/{print $2}')
SHELL=/bin/bash ./target/release/lolidesk host run --listen 127.0.0.1:10190 &
( sleep 2; printf 'echo HELLO-$((6*7))\n'; sleep 2; printf 'exit\n'; sleep 2 ) \
  | ./target/release/lolidesk 127.0.0.1 --token "$TOKEN" -y | grep -a HELLO-42
kill %1
```

The remote desktop can't be tested in CI: the runner has no Screen Recording permission. `core`'s tests do encode a frame with VideoToolbox and decode it back into an NV12 pixel buffer.

## Packaging

CI packs `lolidesk-<macos|linux>-<aarch64|x86_64>.tar.gz` with `lolidesk`, `libghostty-vt.dylib` (`.so` on Linux) and `BUILD-INFO` at the root, plus `Lolidesk.app` on macOS (signed ad hoc, with its own copy of both). A release is manual (run `desk` on `main` with *release* ticked) and never takes `releases/latest`.
