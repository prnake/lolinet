# lolidesk

A remote terminal and remote desktop for machines on a lolinet network. Every machine runs the same program, `lolidesk`: `lolidesk host` lets others reach this machine, and `lolidesk <host>` reaches another one. The desktop app, `Lolidesk.app`, carries the same program. Both sides talk directly over the overlay (`10.188.0.0/16` in the example config), so there is no relay, no account and no public service.

The remote desktop follows Vibe42's model and the terminal follows [kero](https://github.com/egoist/kero)'s (MIT). lolidesk keeps only what remote coding inside your own network needs.

| | |
| :-- | :-- |
| **Terminal** | Both ends run [libghostty-vt](https://github.com/ghostty-org/ghostty), Ghostty's terminal core. The host owns the session: a PTY plus a headless libghostty-vt terminal, the only one that lays out the program's output. Each attached client gets *Mirror frames*: escape sequences that bring the client's own terminal from the last frame to what the host's shows now, cell for cell, scrollback, cursor, modes and title included. A frame goes out about 4 ms after output, so a program's redraw arrives in one piece. Nothing is lost on disconnect, several clients can watch one session, and reconnecting is another Attach whose first frame starts the client over. The client encodes keys, mouse and paste itself, for the modes the frames set (legacy, xterm modifyOtherKeys, kitty keyboard protocol). A session has one size: that of the client the user last typed in or switched to |
| **Persistence** | Every 10 seconds, and when it is stopped, the host saves each session's command, working directory, size and history (scrollback and screen down to the cursor) under its config directory. When stopped it also closes its connections, so clients notice at once and reconnect. After `lolidesk host` restarts (an upgrade, a reboot), each session comes back with its history and a new shell in the same directory, marked "已恢复". The programs themselves are not started again. A program that exits with status 0 closes its session; one that fails leaves the session and its output until you end it |
| **Agents** | "New" offers a plain shell and every coding agent the host's login shell can find (`claude`, `codex`, `gemini`, `opencode`, …), or any command. The command runs in a login shell; when it exits you are left in that shell |
| **Desktop** (macOS host) | ScreenCaptureKit capture → VideoToolbox hardware H.264 (low-latency settings, cursor not drawn into the picture, idle frames not encoded) → QUIC datagram fragments → VideoToolbox hardware decode on the client into NV12 pixel buffers backed by IOSurfaces, which the app draws as GPU textures with no CPU copy. Resolution, frame rate and quality are chosen in the app and can be changed while connected. A lost frame is recovered with a keyframe. Keys are sent by physical position, so the remote's own keyboard layout and input method apply |
| **Transport** | One QUIC connection (quinn). Control, terminal and desktop each open their own stream; video goes in datagrams. The host has a self-signed certificate. A client meeting a new host shows its fingerprint first and sends the token only after you trust it (TOFU); after that the fingerprint is pinned. A shared token authenticates the client |

## Components

| Path | Builds | Contents |
| :-- | :-- | :-- |
| `desk/proto/` | library | Messages (postcard), datagram fragment header, video settings, Catseye palette |
| `desk/ghostty/` | library | Bindings to libghostty-vt, loaded at run time; `Mirror` and `History`, ported from [mygo](https://github.com/egoist/mygo)'s terminal plugin (MIT); render state, key / mouse / paste encoders for the client |
| `desk/host/` | library | `lolidesk host`: QUIC server, terminal sessions and their persistence, agent detection, macOS capture and encoding (`desktop/mac.rs`), input injection (`input.rs`), LaunchAgent install |
| `desk/core/` | library | Client side: connection and fingerprint pinning, remembered hosts, VideoToolbox decoding into NV12 pixel buffers |
| `desk/client/` | `lolidesk` | The one program: `host` (from `desk/host/`), and as a client it attaches a session in the current terminal (TUI) and lists sessions, displays and agents |
| `desk/app/` | `Lolidesk.app` | The desktop app, in Rust with [GPUI](https://github.com/zed-industries/zed) and [gpui-component](https://github.com/longbridge/gpui-kit). It draws the terminal itself over libghostty-vt and shows the remote desktop's decoded frames as GPU textures, with no copy |

## Use

### Host

On the machine you want to reach. Join it to lolinet first. Once a lolidesk release is published, `install.sh` puts `lolidesk` in `/usr/local/lib/lolidesk/` (linked from `/usr/local/bin/lolidesk`) and `lolinet upgrade` keeps it current (see [Distribution](#distribution)). It doesn't start the host. Run these as your own user, not with `sudo`:

```sh
lolidesk host token       # prints the token and certificate fingerprint
lolidesk host run         # runs in the foreground, listening on <this machine's 10.188.x.x>:10190
lolidesk host install     # macOS: install as the current user's LaunchAgent co.lo-li.lolidesk, and start it
lolidesk host uninstall   # macOS: stop and remove it
```

`lolidesk host` with no subcommand is `run`. `Lolidesk.app` has the same program at `Lolidesk.app/Contents/Helpers/lolidesk`; installing the service from there makes it follow the app's updates.

| | |
| :-- | :-- |
| **Config** | Created on first use in `~/Library/Application Support/lolidesk/host/` (macOS) or `~/.config/lolidesk/host/`: certificate, key and token, and `sessions/` (`state.json` and one `<id>.vt` history per session, mode 600). `host token --reset` makes a new token and certificate |
| **Listen address** | `host run --listen <addr:port>` overrides it. With no `10.188.0.0/16` address on the machine, the host listens on `0.0.0.0:10190` and only the token protects it |
| **Terminal core** | `lolidesk` loads `libghostty-vt.dylib` (`.so` on Linux) from beside itself, as the package puts it, or from `../Frameworks` inside the app; `LOLIDESK_GHOSTTY_VT` names another file. Without it the host still serves the desktop, and terminal requests get an error saying so. Programs in a session see `TERM=xterm-256color`, `TERM_PROGRAM=lolidesk` and `LOLIDESK_SESSION=<id>` |
| **Service** | `host install` / `host uninstall`, macOS only. The LaunchAgent runs `<this binary> host run`; installing again restarts it, and moves it to the binary you ran `install` from. Logs go to `~/Library/Logs/lolidesk/host.log`. On Linux, write your own `systemd --user` unit that runs `lolidesk host run` |
| **From `lolidesk-host`** | Earlier packages had a separate `lolidesk-host`. Called by that name, `lolidesk` behaves as `lolidesk host`, and `install.sh` and `lolinet upgrade` turn an installed `/usr/local/bin/lolidesk-host` into a link to it, so an existing LaunchAgent keeps starting. Run `lolidesk host install` once to point the LaunchAgent at `lolidesk` itself |
| **Permissions** | The remote desktop needs two permissions for the `lolidesk` binary the service runs, in System Settings → Privacy & Security: **Screen Recording** (capture) and **Accessibility** (mouse and keyboard injection). Without Screen Recording, the app shows the error under the host; without Accessibility you can watch but not control |

### Desktop app

Open `Lolidesk.app` (in the macOS package; it is signed ad hoc, so the first time right-click it and choose Open).

- **＋** at the top of the sidebar adds a host: its overlay address or name, and the token. The app then shows the host's certificate fingerprint; compare it with what `lolidesk host token` printed and trust it. Remembered hosts connect when the app starts.
- A host that can't be reached, or whose connection drops (a laptop going to sleep, a host restart), is retried after 1, 2, 4 … up to 60 seconds, and at once when the window becomes active; right-click it to retry now or stop. Open terminals stay on screen, dimmed, and are attached again when the host is back. A host that refuses the token or shows a different certificate is not retried.
- Under a host: its sessions (a green icon marks those open in this window), **新建** (new) with the shell, the agents found on the host and a custom command, and its displays. Right-click a host or session for disconnect, end session and forget.
- **Terminal**: click a session to attach; sessions you opened stay attached while you look at something else, and switching back is instant. The close button detaches and leaves the session running on the host; "end session" in the ⋯ menu kills it. Cmd+= / Cmd+- change the font size, Cmd+C / Cmd+V copy and paste, Cmd+K clears the screen on the host.
- **Desktop**: click a display. Click the picture to give it the keyboard (the top bar says so): every key, Tab and Shift+Tab included, goes to the remote, and Cmd+C / V / X / A / Z become the remote's shortcuts. Click outside the picture to take the keyboard back; held keys are released. The toolbar at the bottom chooses the display, the resolution (native or a long edge of 3840 / 2560 / 1920 / 1600 / 1280 / 960 pixels, never above native), the frame rate (15 / 30 / 60 / 120, capped at the display's refresh rate) and the quality (`省流` / `标准` / `清晰` / `极清`, about 0.05 / 0.1 / 0.18 / 0.3 bits per pixel per frame), asks for a fresh picture, toggles full screen, shows the live frame rate, bitrate and round trip, and disconnects. The choice is remembered in `lolidesk/app.json` under the system config directory, with the terminal font size and `option_as_alt` (Option sends Alt to programs; on by default).

### Command line

```sh
lolidesk <host> --token <token>      # first time: check the fingerprint, then it's remembered
lolidesk <host>                      # after that, no token; opens a new session in this terminal
lolidesk <host> -s 3                 # attach to session 3
lolidesk <host> -c claude --cwd ~/p  # new session running a command in a directory
lolidesk <host> --sessions           # list the host's sessions
lolidesk <host> --displays           # list the host's displays (and check Screen Recording)
lolidesk <host> --launchers          # list the agents the host can start
lolidesk --list                      # remembered hosts
lolidesk --forget <host>:10190       # forget a host's fingerprint and token
```

`<host>` is an overlay address or host name, with an optional port (default 10190). The host's frames go straight to your terminal, which encodes your keys itself. Ctrl-] detaches; the session stays on the host. `-y` trusts a new host's fingerprint without asking. Remembered hosts are kept in `lolidesk/hosts` under the system config directory (`~/Library/Application Support` on macOS, `~/.config` on Linux), mode 600, shared by the app and the command line.

## Protocol

Defined in `desk/proto/src/lib.rs`. Protocol version 2, ALPN `lolidesk/1`, TLS server name `lolidesk`. The ALPN stays the same across versions and `Hello` / `Denied` never change their encoding, so a client and host of different versions get a readable "protocol version mismatch" instead of a failed handshake.

- **Streams.** The first byte of every bidirectional stream is its kind: `0` control, `1` terminal, `2` desktop. After that come messages framed as `u32 LE length + postcard`, at most 16 MiB each.
- **Control.** `Hello{version, token, client}` → `Welcome{host, os, version, desktop, persistent}` or `Denied`. Then `ListDisplays` (id, pixels, refresh rate), `ListSessions` (id, title, command, foreground program, working directory, size, viewers, start time, restored, exit status) and `ListLaunchers` (id, name, command).
- **Terminal.** The first message is `Open{size, command?, cwd?}` or `Attach{id, size}`, where a size is columns, rows and a cell's width and height in pixels. The host answers `Opened{id}`, `Resized{cols, rows}` (the session's size) and a first `Frame` that starts the client's terminal over. Then come `Frame`s (Mirror output), `Resized` between the frames of the old size and those of the new, `Bell`, `Notify{title, body}` and `Exited(code)`. The client sends `Input(bytes)` (already encoded for the terminal's modes), `Resize(size)` (what its view fits), `Claim` (the user is here: take my size), `Clear(Screen|Scrollback)` and `Kill`. Typing also claims. A client whose queue backs up is dropped; reattaching starts it over. `cwd` may start with `~/` or be relative to the home directory.
- **Desktop.** `Start{display, video{long_edge, fps, quality}}` → `Ready{width, height, fps, bitrate}`, the settings the host actually used (no upscaling, frame rate capped at the refresh rate). `Configure(video)` restarts capture with new settings and answers with a new `Ready`. Then `Keyframe`, `Input` and `Stop`. Input coordinates are normalised to 0..1 of the picture. `Key{code, down, mods}` carries a physical key by its W3C `KeyboardEvent.code` name; modifiers are pressed and released as keys of their own, and `mods` lets the host correct what it thinks is held. `Scroll{dx, dy}` is a fraction of the picture's height, positive when the content moves down or right. `Text` injects composed text as Unicode; `ReleaseAll` lets go of everything when the client loses focus.
- **Datagrams.** `D5 | frame u32 | index u16 | count u16 | flags u8` (little-endian) followed by a fragment. Joined, the fragments are a `VideoFrame`: `key u8 [sps_len u16 sps pps_len u16 pps] avcc`. AVCC is NAL units with 4-byte big-endian length prefixes. A frame that doesn't fit the host's datagram send buffer is dropped whole, the next ones are skipped until a keyframe, and the encoder is asked for one.

## Distribution

lolidesk ships as its own package and is not built into `lolinet`. `lolinet` stays a small static binary (on Linux it is statically linked against musl, so it runs as is on routers and in containers). lolidesk links ScreenCaptureKit's Swift bridge and the system Swift runtime, and the app is a GUI bundle, so they can't follow those rules. Merging them would break both.

| | |
| :-- | :-- |
| **Package** | `lolidesk-<macos\|linux>-<aarch64\|x86_64>.tar.gz`, with `lolidesk`, `libghostty-vt.dylib` (`.so`) and `BUILD-INFO` at the root, and `Lolidesk.app` too in the macOS packages, with its own copy of both in `Contents/Helpers` and `Contents/Frameworks`: the same format as the lolinet package |
| **Version** | `lolidesk --version` prints the version set in `LOLIDESK_VERSION` at build time. CI sets it to `git describe --always` (the short commit hash), so it matches the first line of `BUILD-INFO` and the manifest's `version`; `lolinet upgrade` compares against it. A local build without it prints the Cargo version |
| **libghostty-vt** | Pinned by Ghostty commit and SHA-256 in `desk/ghostty/libghostty-vt.json`; `desk/scripts/fetch-ghostty.sh <platform> <dir>` downloads and checks it. The builds are the ones mygo publishes (macOS 13+; Linux with glibc 2.28+) |
| **CI** | [`desk.yml`](../.github/workflows/desk.yml) builds the same four platforms as lolinet on pull requests and on `main`: macOS arm64 / x86_64 (with the app) and Linux x86_64 / arm64 (terminal only; arm64 builds natively on GitHub's `ubuntu-24.04-arm` runner). It runs terminal sessions over loopback, including one that has to come back with its history after a host restart |
| **Release** | Automatic: every push to `main` that changes `desk/` (other than `.md` files) publishes a `lolidesk-<version>` GitHub Release once all four builds pass, with the packages, their `.sha256`, `SHA256SUMS` and the manifest `lolidesk.json`. A tag that already exists is skipped, and running `desk` on `main` with *release* ticked publishes again by hand. The release never takes `latest`, because lolinet's auto-upgrade reads `releases/latest` |
| **Manifest** | `lolidesk.json`, written by `lolinet/release.py --lolidesk`: the same shape as `lolinet.json`, keyed `lolidesk`. See [Release artifacts](reference.md#release-artifacts) |
| **Join site** | The [`dist`](../.github/workflows/dist.yml) workflow copies each release to the join site's `dl/`, packages first and `lolidesk.json` last, when the repository has the Cloudflare secrets ([Self-hosting](self-hosting.md#7--upgrade-devices)). By hand: `uv run join/deploy.py lolidesk <dir>`, with the release's files in `<dir>` |
| **Install** | `install.sh` puts `lolidesk` and libghostty-vt in `/usr/local/lib/lolidesk/` and links `/usr/local/bin/lolidesk` to it, both when it pairs a new device and when it upgrades one (`sudo sh` without a code). An older install's `/usr/local/bin/lolidesk-host` becomes a link too: called by that name the program is `lolidesk host`, so an existing LaunchAgent keeps working. It reads `lolidesk.json` from where it got `lolinet`: on GitHub the newest `lolidesk-*` release, found through the GitHub API because these releases don't take `latest`, else the join site's `dl/`. It checks the sha256 and runs the new `lolidesk --version`, which must print the manifest's version. Any problem skips lolidesk and doesn't affect joining. The host service isn't started |
| **Updates** | `lolinet upgrade`, every 6 hours while auto-upgrade is on, replaces `/usr/local/lib/lolidesk/` when the installed `lolidesk --version` differs from `lolidesk.json` (or when `/usr/local/bin` still holds the old separate binaries). It reads the manifest from the same source as `lolinet.json`: from the same directory for the join site or a manifest URL, from the newest `lolidesk-*` release for a `github:` source. The checks are the same as for `lolinet`: sha256, then a test run that must print the manifest's version. It never installs lolidesk where it isn't, and it never restarts a running host, because terminal sessions live in the host process. A running host keeps the old version until it next starts |
| **Build on macOS** | Xcode 26 or later (macOS 26 SDK) for the host: the capture library's Metal bridge uses types from the new SDK. `make desk-app` builds the app and bundles it with `desk/scripts/bundle-app.sh` into `desk/app-out/Lolidesk.app`. GPUI comes from the `gpui-pre` snapshot that `gpui-kit` pins; a first build takes a few minutes |

**macOS permissions after an update.** macOS ties Screen Recording and Accessibility to the binary they were granted to. An updated `lolidesk` is a different binary, and on Apple silicon, where it is ad-hoc signed and identified by its hash, macOS may stop honoring the earlier grants. If the desktop then reports that Screen Recording is missing, or input stops working, remove `lolidesk` from both lists in System Settings → Privacy & Security, add it again, and restart the host (`lolidesk host uninstall && lolidesk host install`). `sudo lolinet up --auto-upgrade off` keeps a machine on its current versions, `lolinet`'s included.

## Roadmap

- Capture on Linux / Windows hosts (PipeWire / DXGI) and client decoding off macOS. On anything but macOS, both sides are terminal only for now.
- Remote clipboard sync, file transfer, several displays at once, an application menu, remote unlock.
- Adaptive bitrate: today the bitrate follows the chosen quality, and congestion only drops frames and asks for a keyframe. FEC.
- A signed `lolidesk` and `Lolidesk.app` (Developer ID), so macOS keeps its permissions across updates.
- Resuming agents' conversations after a host restart (Claude Code, Codex and others report their session id through a SessionStart hook, as kero does) and showing each session's status (working, waiting for you, idle) in the sidebar.
- Kitty graphics in Mirror frames; a GPUI client on Linux.
