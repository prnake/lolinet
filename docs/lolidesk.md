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

On the machine you want to reach. Join it to lolinet first.

```sh
cargo build --release                    # in desk/; builds target/release/lolidesk-host
./target/release/lolidesk-host token     # prints the token and certificate fingerprint
./target/release/lolidesk-host run       # runs in the foreground, listening on <this machine's 10.188.x.x>:10190
./target/release/lolidesk-host install   # macOS: install as the current user's LaunchAgent co.lo-li.lolidesk
```

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
| **CI** | [`desk.yml`](../.github/workflows/desk.yml) builds macOS arm64 / x86_64 and Linux x86_64 (terminal only) on pull requests and on `main` |
| **Release** | Manual: run `desk` on `main` with *release* ticked. It publishes a `lolidesk-<version>` GitHub Release that does not take `latest`, because lolinet's auto-upgrade reads `releases/latest` |
| **Join site** | `uv run join/deploy.py dist <tarball>…` uploads the same packages (with `.sha256`) to the join site's download area |
| **Build on macOS** | Needs Xcode 26 or later (macOS 26 SDK). The capture library's Metal bridge uses types from the new SDK, and older Xcode can't build it |

The install script doesn't install lolidesk yet. Download and unpack it by hand for now.

## Roadmap

- Capture on Linux / Windows hosts (PipeWire / DXGI) and client decoding (VA-API / Media Foundation). On anything but macOS, both sides are terminal only for now.
- Remote clipboard sync, file transfer, several displays at once, an application menu, remote unlock.
- Adaptive bitrate. The bitrate is fixed per quality level today, and a lost frame only asks for a keyframe; it doesn't step down. FEC.
- GUI font: egui's built-in monospace font for now. Switch to Geist Mono (already in `web/`) with a Nerd Font fallback.
