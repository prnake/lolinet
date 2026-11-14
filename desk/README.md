# desk/

lolidesk: a remote terminal and remote desktop over the lolinet overlay. A Rust workspace, built and released on its own, never linked into `lolinet`. Usage, protocol and distribution: [docs/lolidesk.md](../docs/lolidesk.md).

| Path | Builds | Contents |
| :-- | :-- | :-- |
| `proto/` | library | Wire protocol: messages (postcard), datagram fragment header, video settings, palette |
| `ghostty/` | library | libghostty-vt bindings (loaded at run time), `Mirror` and `History`, render state and input encoders. See [ghostty/README.md](ghostty/README.md) |
| `host/` | library | `lolidesk host`: QUIC server, terminal sessions (PTY + headless libghostty-vt) and their persistence, pairing and access (`src/access.rs`), agent detection, macOS capture and encoding (`src/desktop/mac.rs`), input injection (`src/input.rs`), service install (a LaunchAgent on macOS, a `systemd --user` unit on Linux) |
| `core/` | library | Client side: connection and fingerprint pinning, remembered hosts, VideoToolbox decoding into NV12 pixel buffers |
| `client/` | `lolidesk` | The one program: `lolidesk host …` (from `host/`) and the client side, command line and TUI |
| `app/` | `lolidesk-app` → `Lolidesk.app` (macOS), `lolidesk-app` in the package (Linux) | Desktop app: GPUI + gpui-component, macOS and Linux (a stub elsewhere). On Linux everything but the remote desktop picture: there is no decoder, so that page says so. Pairing lives in `src/ui/access.rs` (the access page, new codes), `src/ui/pair.rs` (consent prompt, connecting, share links) and `src/service.rs` (this computer's host service, notifications, finding devices, behind a trait the tests fake). `app/linux/lolidesk.desktop` is the application-menu template; it registers `lolidesk://` share links |
| `scripts/` | | `fetch-ghostty.sh` (the pinned libghostty-vt for a platform), `fetch-mihomo.sh` (the pinned mihomo, `mihomo/mihomo.json`), `bundle-app.sh` (Lolidesk.app, with `lolidesk` in `Contents/Helpers`: `Contents/MacOS/lolidesk` would overwrite `Lolidesk` on a case-insensitive disk; with `MIHOMO=<path>` set, mihomo goes next to it; its `Info.plist` registers the `lolidesk` URL scheme for share links), `bundle-linux.sh` (adds `lolidesk-app`, `lolidesk.desktop` and `lolidesk.svg` to a Linux package directory), `bundle-dmg.sh` (Lolidesk.app on a drag-to-Applications disk image, laid out by dmgbuild from `dmg-settings.py`, pinned in `dmgbuild.txt`), `dmg-background.py` (draws the image's background into `app/macos/`) |
| `mihomo/` | | `mihomo.json`: the mihomo release the app ships (version, per-platform URL and SHA-256 of the `.gz`, GPL-3.0, source link) |

## Build and check

Run from the repository root:

```sh
make check-desk     # fetches libghostty-vt into desk/.ghostty/, then fmt, clippy and tests
make desk           # release builds in desk/target/release/
make desk-app       # desk/app-out/Lolidesk.app (macOS) or desk/app-out/lolidesk/ (Linux, the package layout)
make desk-dmg       # macOS: desk/app-out/Lolidesk.dmg (needs uv)
```

Or by hand from `desk/`, which is what [desk.yml](../.github/workflows/desk.yml) runs on every pull request:

```sh
sh scripts/fetch-ghostty.sh darwin-arm64 .ghostty          # or darwin-amd64, linux-amd64, linux-arm64
export LOLIDESK_GHOSTTY_VT=$PWD/.ghostty/libghostty-vt.dylib
cargo fmt --all --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
cargo build --locked --release --workspace
sh scripts/bundle-app.sh target/release/lolidesk-app target/release/lolidesk "$LOLIDESK_GHOSTTY_VT" app-out   # macOS
sh scripts/bundle-linux.sh target/release/lolidesk-app <package dir>                                       # Linux
```

| | |
| :-- | :-- |
| **macOS** | Xcode 26 or later (macOS 26 SDK). The capture library's Metal bridge uses types from that SDK. CI sets `DEVELOPER_DIR` to Xcode 26 on the `macos-15` image. The first build of the app compiles GPUI and takes a few minutes |
| **Linux** | The host and command line need no extra system packages. The app (GPUI) needs `pkg-config libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libxcb1-dev libx11-xcb-dev libfontconfig-dev libfreetype-dev libvulkan-dev` (Debian / Ubuntu); `cargo clippy` / `test --workspace` build it too. Its snapshot tests draw with wgpu, so they need a Vulkan driver (`mesa-vulkan-drivers` gives a software one) and skip without one. To type-check it from macOS: `RUST_FONTCONFIG_DLOPEN=1 cargo zigbuild clippy --target x86_64-unknown-linux-gnu …` (fontconfig's build script can't run pkg-config across targets; linking still needs Linux) |
| **Targets in CI** | `aarch64-apple-darwin`, `x86_64-apple-darwin`, `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu` (on the `ubuntu-24.04-arm` runner) |
| **libghostty-vt** | Loaded at run time from `$LOLIDESK_GHOSTTY_VT`, beside the executable (the Linux package keeps it next to `lolidesk-app`), or the app's `Contents/Frameworks`. Tests that need it print a skip message without it |
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

CI builds with `LOLIDESK_VERSION=$(git describe --always)`, so `--version` prints the commit, and packs `lolidesk-<macos|linux>-<aarch64|x86_64>.tar.gz` with `lolidesk`, `libghostty-vt.dylib` (`.so` on Linux) and `BUILD-INFO` at the root, plus `Lolidesk.app` on macOS (also as `Lolidesk-macos-<arm64|x86_64>.dmg`; signed ad hoc, with its own copy of both, and mihomo in `Contents/Helpers/mihomo` with a GPL notice in `Contents/Resources/mihomo-NOTICE.txt`), and `lolidesk-app`, `lolidesk.desktop` and `lolidesk.svg` on Linux (the app treats a directory with `BUILD-INFO` and `lolidesk` beside it as the package; `ldd` must find all its libraries). On Linux CI also uploads the snapshot PNGs as `lolidesk-snapshots-<platform>`. Every push to `main` that changes `desk/` (other than `.md` files) publishes a `lolidesk-<commit>` release with the manifest `lolidesk.json` once all four builds pass. It never takes `releases/latest`. `install.sh` installs from it and `lolinet upgrade` keeps an installed lolidesk at its version. See [Distribution](../docs/lolidesk.md#distribution).
