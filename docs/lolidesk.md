# lolidesk

A remote terminal and remote desktop for machines on a lolinet network. One host process, one client binary. Both sides talk directly over the overlay (`10.188.0.0/16` in the example config), so there is no relay, no account and no public service.

Some design choices follow Vibe42's remote-terminal model. lolidesk keeps only what remote coding inside your own network needs.

| | |
| :-- | :-- |
| **Terminal** | The host owns the session: a PTY plus an `alacritty_terminal` model. A client that connects gets a full-screen snapshot first, then the byte stream. Nothing is lost on disconnect, several clients can watch one session, and reconnecting is just another Attach. The GUI client sends *semantic keys*, and the host encodes them for the terminal's current mode (DECCKM, keypad application mode, bracketed paste). The TUI client passes local bytes through as they are |
| **Desktop** (macOS host) | ScreenCaptureKit capture → VideoToolbox hardware H.264 (low-latency settings, cursor not drawn into the picture, idle frames not encoded) → QUIC datagram fragments → VideoToolbox hardware decode on the client → egui texture. A lost frame is recovered by the client asking for a keyframe. Mouse and keyboard are injected with CGEvent; text is injected as Unicode, so it doesn't depend on the keyboard layout |
| **Transport** | One QUIC connection (quinn). Control, terminal and desktop each open their own stream; video goes in datagrams. The host has a self-signed certificate. The client checks its fingerprint the first time it connects, remembers it (TOFU) and pins it after that. A shared token authenticates the client |

## Crates

| Crate | Builds | Contents |
| :-- | :-- | :-- |
| `desk/proto/` | library | Messages (postcard), datagram fragment header, semantic key encoding, Catseye palette |
| `desk/host/` | `lolidesk-host` | QUIC server, PTY sessions, macOS capture and encoding (`desktop/mac.rs`), input injection (`input.rs`), LaunchAgent install |
| `desk/client/` | `lolidesk` | GUI (egui + wgpu: sessions and displays in the left column, the terminal grid or desktop picture in the middle) and TUI (`--tui`, attaches a session in the current terminal) |

## Use

### Host

On the machine you want to reach. Join it to lolinet first. Once a lolidesk release is published, `install.sh` puts `lolidesk-host` and `lolidesk` in `/usr/local/bin` and `lolinet upgrade` keeps them current (see [Distribution](#distribution)). It doesn't start the host. Run these as your own user, not with `sudo`:

```sh
lolidesk-host token     # prints the token and certificate fingerprint
lolidesk-host run       # runs in the foreground, listening on <this machine's 10.188.x.x>:10190
lolidesk-host install   # macOS: install as the current user's LaunchAgent co.lo-li.lolidesk
```

To build it yourself instead, run `cargo build --release` in `desk/` and use `target/release/lolidesk-host`.

| | |
| :-- | :-- |
| **Config** | Created on first use in `~/Library/Application Support/lolidesk/host/` (macOS) or `~/.config/lolidesk/host/`. `token --reset` makes a new token and certificate |
| **Listen address** | `run --listen <addr:port>` overrides it. With no `10.188.0.0/16` address on the machine, the host listens on `0.0.0.0:10190` and only the token protects it |
| **Service** | `install` / `uninstall`, macOS only. Logs go to `~/Library/Logs/lolidesk/host.log`. On Linux, write your own `systemd --user` unit that runs `lolidesk-host run` |
| **Permissions** | The remote desktop needs two permissions for the `lolidesk-host` binary, in System Settings → Privacy & Security: **Screen Recording** (capture) and **Accessibility** (mouse and keyboard injection). Without Screen Recording, `ListDisplays` / `Start` return an error that says so. Without Accessibility you can watch but not control |

### Client

```sh
lolidesk <host> --token <token>      # first time: check the fingerprint, then it's remembered
lolidesk <host>                      # after that, no token; opens a window with a new terminal
lolidesk <host> -s 3                 # attach to session 3
lolidesk <host> -d 0 -q sharp        # remote desktop on the main display, sharp quality
lolidesk <host> --tui                # no window: attach in this terminal; Ctrl-] detaches (the session stays on the host)
lolidesk <host> --sessions           # list the host's sessions
lolidesk <host> --displays           # list the host's displays (and check Screen Recording)
lolidesk --list                      # remembered hosts
lolidesk --forget <host>:10190       # forget a host's fingerprint and token
```

`<host>` is an overlay address or host name, with an optional port (default 10190). `-y` trusts a new host's fingerprint without asking. Remembered hosts are kept in `lolidesk/hosts` under the system config directory (`~/Library/Application Support` on macOS, `~/.config` on Linux), mode 600.

In the GUI (its labels are in Chinese):

- **＋ 新终端** (new terminal) in the left column, or click an existing session. Click a display to open the desktop.
- Three quality levels, **流畅 / 均衡 / 清晰** (`smooth` / `balanced` / `sharp` on the command line). They scale the long edge to 1280, 1920 or native pixels, at about 4, 10 or 24 Mbps for 1080p. The default is `balanced`.
- **刷新画面** (refresh picture) asks for a keyframe.
- Click the terminal area to give it the keyboard. Cmd+C / Cmd+V use the system clipboard; paste goes through bracketed paste.

## Protocol

Defined in `desk/proto/src/lib.rs`. Protocol version 1, ALPN `lolidesk/1`, TLS server name `lolidesk`.

- **Streams.** The first byte of every bidirectional stream is its kind: `0` control, `1` terminal, `2` desktop. After that come messages framed as `u32 LE length + postcard`, at most 16 MiB each.
- **Control.** `Hello{version, token, client}` → `Welcome{host, os, version, desktop}` or `Denied`. Then `ListDisplays`, `ListSessions`.
- **Terminal.** The first message is `Open{cols, rows, command?, cwd?}` or `Attach{id, cols, rows}`. The host answers `Opened{id}` and `Snapshot{cols, rows, data}`, then `Output` / `Title` / `Bell` / `Exited`. The client sends `Input(Key|Text|Paste|Raw)`, `Resize` and `Kill`. After a `Resize`, the host sends every watcher a new snapshot: the two sides' models would drift apart if each reflowed on its own, so the host's is the one that counts.
- **Desktop.** `Start{display, quality, max_fps}` → `Ready{width, height}`. Then `Keyframe`, `Quality`, `Input(Move|Button|Scroll|Key|Text)` with coordinates normalised to 0..1, and `Stop`.
- **Datagrams.** `D5 | frame u32 | index u16 | count u16 | flags u8` (little-endian) followed by a fragment. Joined, the fragments are a `VideoFrame`: `key u8 [sps_len u16 sps pps_len u16 pps] avcc`. AVCC is NAL units with 4-byte big-endian length prefixes.

## Distribution

lolidesk ships as its own package and is not built into `lolinet`. `lolinet` stays a small static binary (on Linux it is statically linked against musl, so it runs as is on routers and in containers). lolidesk links ScreenCaptureKit's Swift bridge, the system Swift runtime and wgpu, so it can only be linked dynamically and is an order of magnitude larger. Merging it would break both.

| | |
| :-- | :-- |
| **Package** | `lolidesk-<macos\|linux>-<aarch64\|x86_64>.tar.gz`, with `lolidesk-host`, `lolidesk` and `BUILD-INFO` at the root: the same format as the lolinet package |
| **Version** | `lolidesk-host --version` and `lolidesk --version` print the version set in `LOLIDESK_VERSION` at build time. CI sets it to `git describe --always` (the short commit hash), so it matches the first line of `BUILD-INFO` and the manifest's `version`; `lolinet upgrade` compares against it. A local build without it prints the Cargo version |
| **CI** | [`desk.yml`](../.github/workflows/desk.yml) builds the same four platforms as lolinet on pull requests and on `main`: macOS arm64 / x86_64 and Linux x86_64 / arm64 (Linux is terminal only). Linux arm64 builds natively on GitHub's `ubuntu-24.04-arm` runner |
| **Release** | Automatic: every push to `main` that changes `desk/` (other than `.md` files) publishes a `lolidesk-<version>` GitHub Release once all four builds pass, with the packages, their `.sha256`, `SHA256SUMS` and the manifest `lolidesk.json`. A tag that already exists is skipped, and running `desk` on `main` with *release* ticked publishes again by hand. The release never takes `latest`, because lolinet's auto-upgrade reads `releases/latest` |
| **Manifest** | `lolidesk.json`, written by `lolinet/release.py --lolidesk`: the same shape as `lolinet.json`, keyed `lolidesk`. See [Release artifacts](reference.md#release-artifacts) |
| **Join site** | The [`dist`](../.github/workflows/dist.yml) workflow copies each release to the join site's `dl/`, packages first and `lolidesk.json` last, when the repository has the Cloudflare secrets ([Self-hosting](self-hosting.md#7--upgrade-devices)). By hand: `uv run join/deploy.py lolidesk <dir>`, with the release's files in `<dir>` |
| **Install** | `install.sh` installs `lolidesk-host` and `lolidesk` to `/usr/local/bin`, both when it pairs a new device and when it upgrades one (`sudo sh` without a code). It reads `lolidesk.json` from where it got `lolinet`. On GitHub that is the newest `lolidesk-*` release, found through the GitHub API because these releases don't take `latest`; if GitHub has none, it tries the join site's `dl/`. It checks the sha256 and runs each binary with `--version`, and installs only the ones that run and print the manifest's version. The Linux packages are built on Ubuntu 24.04 and need glibc 2.34 for the host and 2.35 for the client. Any problem skips lolidesk and doesn't affect joining. The host service isn't started |
| **Updates** | `lolinet upgrade`, every 6 hours while auto-upgrade is on, replaces each lolidesk binary in `/usr/local/bin` whose `--version` differs from `lolidesk.json`. It reads the manifest from the same source as `lolinet.json`: from the same directory for the join site or a manifest URL, from the newest `lolidesk-*` release for a `github:` source. The checks are the same as for `lolinet`: sha256, then a test run that must print the manifest's version. It never installs a binary that isn't there, and it never restarts a running host, because terminal sessions live in the host process. A running host keeps the old version until it next starts |
| **Build on macOS** | Needs Xcode 26 or later (macOS 26 SDK). The capture library's Metal bridge uses types from the new SDK, and older Xcode can't build it |

**macOS permissions after an update.** macOS ties Screen Recording and Accessibility to the binary they were granted to. An updated `lolidesk-host` is a different binary, and on Apple silicon, where it is ad-hoc signed and identified by its hash, macOS may stop honoring the earlier grants. If the desktop then reports that Screen Recording is missing, or input stops working, remove `lolidesk-host` from both lists in System Settings → Privacy & Security, add it again, and restart the host (`lolidesk-host uninstall && lolidesk-host install`). `sudo lolinet up --auto-upgrade off` keeps a machine on its current versions, `lolinet`'s included.

## Roadmap

- Capture on Linux / Windows hosts (PipeWire / DXGI) and client decoding (VA-API / Media Foundation). On anything but macOS, both sides are terminal only for now.
- Remote clipboard sync, file transfer, several displays at once, an application menu, remote unlock.
- Adaptive bitrate. The bitrate is fixed per quality level today, and a lost frame only asks for a keyframe; it doesn't step down. FEC.
- A signed `lolidesk-host` (Developer ID), so macOS keeps its permissions across updates.
- GUI font: egui's built-in monospace font for now. Switch to Geist Mono (already in `web/`) with a Nerd Font fallback.
