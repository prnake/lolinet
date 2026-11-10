# desk/

lolidesk: a remote terminal and remote desktop over the lolinet overlay. A Rust workspace, built and released on its own, never linked into `lolinet`. Usage, protocol and distribution: [docs/lolidesk.md](../docs/lolidesk.md).

| Crate | Builds | Contents |
| :-- | :-- | :-- |
| `proto/` | library | Wire protocol: messages (postcard), datagram fragment header, semantic key encoding, palette |
| `host/` | `lolidesk-host` | QUIC server, PTY sessions, macOS capture and encoding (`src/desktop/mac.rs`), input injection (`src/input.rs`), LaunchAgent install |
| `client/` | `lolidesk` | GUI (egui + wgpu) and TUI (`--tui`) |

## Build and check

Run from `desk/`. These are the steps [desk.yml](../.github/workflows/desk.yml) runs on every pull request:

```sh
cargo fmt --all --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
cargo build --locked --release --workspace    # target/release/lolidesk-host, target/release/lolidesk
```

| | |
| :-- | :-- |
| **macOS** | Xcode 26 or later (macOS 26 SDK). The capture library's Metal bridge uses types from that SDK. CI sets `DEVELOPER_DIR` to Xcode 26 on the `macos-15` image |
| **Linux** | Terminal only. egui / wgpu need `libxkbcommon-dev libwayland-dev libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev libgtk-3-dev` |
| **Targets in CI** | `aarch64-apple-darwin`, `x86_64-apple-darwin`, `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu` (on the `ubuntu-24.04-arm` runner) |

## Smoke test

CI starts the host on loopback, opens a session from the TUI client with its stdin on a pipe, and runs `echo HELLO-$((6*7))`. The echo only contains `$((6*7))`, so `HELLO-42` in the output proves the command ran in the host's PTY and the bytes came back. A wrong token must be refused. To run it locally:

```sh
TOKEN=$(./target/release/lolidesk-host token | awk '/^token/{print $2}')
SHELL=/bin/bash ./target/release/lolidesk-host run --listen 127.0.0.1:10190 &
( sleep 2; printf 'echo HELLO-$((6*7))\n'; sleep 2; printf 'exit\n'; sleep 2 ) \
  | ./target/release/lolidesk 127.0.0.1 --token "$TOKEN" --tui -y | grep -a HELLO-42
kill %1
```

The remote desktop can't be tested in CI: the runner has no Screen Recording permission.

## Packaging

CI builds with `LOLIDESK_VERSION=$(git describe --always)`, so `--version` prints the commit, and packs `lolidesk-<macos|linux>-<aarch64|x86_64>.tar.gz` with `lolidesk-host`, `lolidesk` and `BUILD-INFO` at the root. Every push to `main` that changes `desk/` (other than `.md` files) publishes a `lolidesk-<commit>` release with the manifest `lolidesk.json` once all four builds pass. It never takes `releases/latest`. `install.sh` installs from it and `lolinet upgrade` keeps installed binaries at its version. See [Distribution](../docs/lolidesk.md#distribution).
