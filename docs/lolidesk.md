# lolidesk

A remote terminal and remote desktop for machines on a lolinet network. Every machine runs the same program, `lolidesk`: `lolidesk host` lets others reach this machine, and `lolidesk <host>` reaches another one. The desktop app (`Lolidesk.app` on macOS, `lolidesk-app` on Linux) carries the same program. Both sides talk directly over the overlay (`10.188.0.0/16` in the example config), so there is no relay, no account and no public service.

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
| `desk/host/` | library | `lolidesk host`: QUIC server, terminal sessions and their persistence, agent detection, macOS capture and encoding (`desktop/mac.rs`), input injection (`input.rs`), service install (LaunchAgent, `systemd --user`) |
| `desk/core/` | library | Client side: connection and fingerprint pinning, remembered hosts, VideoToolbox decoding into NV12 pixel buffers |
| `desk/client/` | `lolidesk` | The one program: `host` (from `desk/host/`), and as a client it attaches a session in the current terminal (TUI) and lists sessions, displays and agents |
| `desk/app/` | `Lolidesk.app` (macOS), `lolidesk-app` (Linux) | The desktop app, in Rust with [GPUI](https://github.com/zed-industries/zed) and [gpui-component](https://github.com/longbridge/gpui-kit). It draws the terminal itself over libghostty-vt and, on macOS, shows the remote desktop's decoded frames as GPU textures, with no copy. On Linux there is no decoder yet: the remote desktop page says so and starts no stream |

## Use

### Host

On the machine you want to reach. Join it to lolinet first. Once a lolidesk release is published, `install.sh` puts `lolidesk` in `/usr/local/lib/lolidesk/` (linked from `/usr/local/bin/lolidesk`) and `lolinet upgrade` keeps it current (see [Distribution](#distribution)). It doesn't start the host. Run these as your own user, not with `sudo`:

```sh
lolidesk host token       # prints the token and certificate fingerprint
lolidesk host run         # runs in the foreground, listening on <this machine's 10.188.x.x>:10190
lolidesk host install     # install as the current user's service and start it (macOS: LaunchAgent co.lo-li.lolidesk; Linux: systemd --user lolidesk.service)
lolidesk host uninstall   # stop and remove it
```

`lolidesk host` with no subcommand is `run`. `Lolidesk.app` has the same program at `Lolidesk.app/Contents/Helpers/lolidesk`; installing the service from there makes it follow the app's updates.

| | |
| :-- | :-- |
| **Config** | Created on first use in `~/Library/Application Support/lolidesk/host/` (macOS) or `~/.config/lolidesk/host/`, all mode 600: certificate, key and token; `access.json` (paired devices and pairing codes, see [Pairing](#pairing-and-access)); `allow.json`; `local` (the loopback address below); and `sessions/` (`state.json` and one `<id>.vt` history per session). `host token --reset` makes a new token and certificate |
| **Listen address** | This machine's `10.188.x.x:10190`, plus `127.0.0.1:10190` for the app and command line on the same machine. `host run --listen <addr:port>` overrides the first. With no `10.188.0.0/16` address the host listens on `0.0.0.0:10190`; devices that aren't paired can still only ask for consent |
| **Terminal core** | `lolidesk` loads `libghostty-vt.dylib` (`.so` on Linux) from beside itself, as the package puts it, or from `../Frameworks` inside the app; `LOLIDESK_GHOSTTY_VT` names another file. Without it the host still serves the desktop, and terminal requests get an error saying so. Programs in a session see `TERM=xterm-256color`, `TERM_PROGRAM=lolidesk` and `LOLIDESK_SESSION=<id>` |
| **Service** | `host install` / `host uninstall`. On macOS a LaunchAgent, `~/Library/LaunchAgents/co.lo-li.lolidesk.plist`, runs `<this binary> host run`; logs go to `~/Library/Logs/lolidesk/host.log`. On Linux a `systemd --user` unit (see *This computer's host* under the desktop app). Installing again restarts it, and moves it to the binary you ran `install` from. Other systems: run `lolidesk host run` yourself |
| **From `lolidesk-host`** | Earlier packages had a separate `lolidesk-host`. Called by that name, `lolidesk` behaves as `lolidesk host`, and `install.sh` and `lolinet upgrade` turn an installed `/usr/local/bin/lolidesk-host` into a link to it, so an existing LaunchAgent keeps starting. Run `lolidesk host install` once to point the LaunchAgent at `lolidesk` itself |
| **Permissions** | The remote desktop needs two permissions for the `lolidesk` binary the service runs, in System Settings → Privacy & Security: **Screen Recording** (capture) and **Accessibility** (mouse and keyboard injection). Without Screen Recording there is no picture; without Accessibility you can watch but not control. Computer use for agents uses the same two grants. The app walks you through both (see [Desktop app](#desktop-app)); `lolidesk <host> --permissions` prints them. Screen Recording only takes effect after the host restarts |
| **What it offers** | `lolidesk host allow` shows and `lolidesk host allow <what> on\|off` changes what the host offers: `terminal`, `desktop` (watch), `control` (input in the remote desktop; off means view-only), `cua` (computer use for agents), `pair` (devices that aren't paired may ask for consent) and `share` (paired devices may make share links). All on by default. A paired device gets this combined with its own limits (see [Pairing](#pairing-and-access)). Stored in `allow.json` in the config directory and read for every new connection and stream, so a change applies without a restart. `Welcome` tells clients what is on |

### Pairing and access

Every computer runs the same program, so each one is both a host and a client. Who may connect to a host is decided on that host:

| | |
| :-- | :-- |
| **Owner** | Whoever has the host's token: the app and command line on the same machine read it from the config directory and connect over `127.0.0.1` as the owner. `--token` still works from elsewhere and also makes you the owner |
| **Paired device** | Every other computer gets its own credential (`g1.<id>.<secret>`; the host keeps only its SHA-256) when it pairs, and uses it from then on without asking again. The host records the device's name, system, how it paired, when, and the time and address of its last connection |
| **Pairing by consent** | `lolidesk pair <host>`, or *连接* in the app on a device in the network: the host asks its owner. The owner's app shows the request; if it isn't open, the host shows a system dialog (macOS `display dialog`, Linux `zenity` or `kdialog`); otherwise `lolidesk host access` lists it and `lolidesk host approve <id>` / `deny <id>` answers. Requests expire after 2 minutes. One pending request per address, at most 5 at a time, and an address is turned away for a minute after a refusal. `LOLIDESK_NO_DIALOG=1` turns the system dialog off |
| **Pairing codes and share links** | `lolidesk host code [--label] [--expires 7d] [--uses N] [--view-only]` on the host prints a 16-character code and a link `lolidesk://pair?h=<address>&f=<certificate fingerprint>&c=<code>&n=<name>`. `lolidesk pair <link>` (or importing it in the app) pairs without asking, and checks the host against the fingerprint in the link. A paired device can make share links for the same host too: `lolidesk share <host>`. A device can't share more than it may do itself. Expiry and the use limit only stop new pairings |
| **Seeing and revoking** | `lolidesk host access` lists the paired devices grouped by how they paired (by consent, then each code with the devices that used it, with their last connection) and the pending requests; `lolidesk access <host>` shows a device its own entry and its share links. `lolidesk host revoke <g-…\|c-…>` revokes a device or a code; revoking cascades (a code takes its devices, a device takes its share links and theirs) and closes their open connections at once. A device may revoke itself and its own share links |
| **Limits** | `lolidesk host limit <g-…> <terminal\|desktop\|control\|cua\|share> on\|off`, or `--reset`. A device may do what the host offers and its limit allows, worked out again for every new stream. Codes made with `--view-only` give their devices a view-only limit |

### Desktop app

Open `Lolidesk.app` (in the macOS package; it is signed ad hoc, so the first time right-click it and choose Open). On Linux, unpack the Linux package anywhere and run `./lolidesk-app` from it (it loads `libghostty-vt.so` from beside itself); GPUI picks X11 or Wayland from the session, and Chinese text needs a CJK font such as Noto Sans CJK. The Linux app is the same app with these differences:

| | Linux |
| :-- | :-- |
| **Remote desktop** | Not yet: there is no video decoder on Linux, so clicking a display shows *远程桌面还不能在 Linux 上看* and asks the host for nothing. Terminals work as on macOS |
| **Administrator prompts** | `pkexec` (polkit's password dialog) with an argument list, no shell: `pkexec /usr/local/bin/lolinet exit on\|off` (only for a root-owned `/usr/local/bin/lolinet` that group and others can't write) and `pkexec /bin/ln -sfn <package>/lolidesk /usr/local/bin/lolidesk`. Joining runs `pkexec /bin/sh -c <command>`, where the command is the same one macOS runs, built only from the validated, single-quoted join site and code. Closing the dialog counts as cancelled |
| **Command line** | **安装到命令行** links the `lolidesk` next to `lolidesk-app` in the unpacked package. The app counts a directory as the package only when `BUILD-INFO` and `lolidesk` are beside it |
| **Application menu** | **加到应用菜单** (a row under *Command line*) writes `~/.local/share/applications/lolidesk.desktop` (`$XDG_DATA_HOME`): the package's `lolidesk.desktop` with `Exec`, `TryExec` and `Icon` set to the package's absolute paths. No administrator prompt. It is shown as added only while the file matches this package; after moving the package, add it again. By hand: copy `lolidesk.desktop` there and edit those three lines. A path with quotes, `\`, `$`, `` ` `` or `%` is refused |
| **Keys** | The app's own keys use Ctrl+Shift, as in GNOME Terminal, so every Ctrl key reaches the program in the terminal: Ctrl+Shift+C / V copy and paste, Ctrl+Shift+A selects all, Ctrl+Shift+K clears, Ctrl+= / Ctrl+- / Ctrl+0 change the font size, Ctrl+click opens a link. Ctrl+Shift+N adds a host, Ctrl+Shift+Q quits, F11 toggles full screen |
| **Window** | Where the compositor draws no title bar (GNOME on Wayland), the app draws minimize, maximize and close at the right of the top bar. The window's app id (Wayland) and class (X11) are `lolidesk`, matching the desktop file |
| **Permissions** | Linux hosts have no desktop, so no permission card. A macOS host's card shows as on macOS; **在主机上打开设置** opens the settings on that host only |

- **Network**, the first entry in the sidebar, is this Mac in lolinet. Everything on it comes from the `lolinet` command line (`/usr/local/bin/lolinet`, else `PATH`), run in the background; the page refreshes every 3 seconds while it is open.
  - *Not joined*: paste the pairing command (`curl -fsSL https://<join site>/i | sudo sh -s XXXX-XXXX-XXXX`), or enter the join site and the code, optionally ticking *无 TUN (userspace)*. The app accepts only an `https://` host name with an optional port and an optional `/i`, and a 12-character code; it builds the command from those parts alone, shows it, and runs it with an administrator prompt (`osascript … with administrator privileges`). The script's output is shown when it is done.
  - *Joined*: this node (name, address, TUN or userspace), the other nodes with direct or relayed, the tunnel and the latency (from `lolinet cli -o json node|peer`), and a switch for the [exit node](design.md#exit-nodes) (`lolinet exit on|off` with an administrator prompt; on while `/etc/lolinet/exit.json` exists).
  - **邀请** (invite) runs `lolinet invite --json --kind desktop|userspace|phone` and shows the code, the command to copy, and for a phone a QR code of the `<join>/m#CODE` link. A code works once, within 15 minutes.
  - *Command line*: whether `/usr/local/bin/lolinet` and `/usr/local/bin/lolidesk` exist. When `lolidesk` doesn't, **安装到命令行** links the app's own `Contents/Helpers/lolidesk` there (administrator prompt). `lolinet` comes with joining.
- **Permissions**: when a connected macOS host is missing Screen Recording or Accessibility, a card under the host (and at the top of its remote desktop) lists what is missing and what each is for. **在主机上打开设置** shows the system prompt on the host and opens the right pane of System Settings there, and here too when the host is this Mac. The app asks the host every 2 seconds while the card is shown, and a row turns green when granted. Once Screen Recording is on, **重启主机服务** restarts the host's LaunchAgent so it takes effect; terminal sessions are saved and come back as after any restart.
- **＋** at the top of the sidebar adds a host: its overlay address or name, and the token. The app then shows the host's certificate fingerprint; compare it with what `lolidesk host token` printed and trust it. Remembered hosts connect when the app starts.
- A host that can't be reached, or whose connection drops (a laptop going to sleep, a host restart), is retried after 1, 2, 4 … up to 60 seconds, and at once when the window becomes active; right-click it to retry now or stop. Open terminals stay on screen, dimmed, and are attached again when the host is back. A host that refuses the token or shows a different certificate is not retried.
- Under a host: its sessions (a green icon marks those open in this window), **新建** (new) with the shell, the agents found on the host and a custom command, and its displays. Right-click a host or session for disconnect, end session and forget.
- **Terminal**: click a session to attach; sessions you opened stay attached while you look at something else, and switching back is instant. The close button detaches and leaves the session running on the host; "end session" in the ⋯ menu kills it. Cmd+= / Cmd+- change the font size, Cmd+C / Cmd+V copy and paste, Cmd+K clears the screen on the host.
- **Desktop**: click a display. Click the picture to give it the keyboard (the top bar says so): every key, Tab and Shift+Tab included, goes to the remote, and Cmd+C / V / X / A / Z become the remote's shortcuts. Click outside the picture to take the keyboard back; held keys are released. The toolbar at the bottom chooses the display, the resolution (native or a long edge of 3840 / 2560 / 1920 / 1600 / 1280 / 960 pixels, never above native), the frame rate (15 / 30 / 60 / 120, capped at the display's refresh rate) and the quality (`省流` / `标准` / `清晰` / `极清`, about 0.05 / 0.1 / 0.18 / 0.3 bits per pixel per frame), asks for a fresh picture, toggles full screen, shows the live frame rate, bitrate and round trip, and disconnects. The choice is remembered in `lolidesk/app.json` under the system config directory, with the terminal font size and `option_as_alt` (Option sends Alt to programs; on by default).

### Command line

```sh
lolidesk pair <host>                 # first time: ask the host to agree (or: lolidesk pair <share link>)
lolidesk <host>                      # after that: opens a new session in this terminal
lolidesk share <host>                # a share link for that host, for another of your machines
lolidesk access <host>               # what that host knows about you and your share links
lolidesk <host> -s 3                 # attach to session 3
lolidesk <host> -c claude --cwd ~/p  # new session running a command in a directory
lolidesk <host> --sessions           # list the host's sessions
lolidesk <host> --displays           # list the host's displays (and check Screen Recording)
lolidesk <host> --launchers          # list the agents the host can start
lolidesk <host> --permissions        # Screen Recording and Accessibility on the host
lolidesk --list                      # remembered hosts
lolidesk --forget <host>:10190       # forget a host's fingerprint and token
```

`<host>` is an overlay address or host name, with an optional port (default 10190). The host's frames go straight to your terminal, which encodes your keys itself. Ctrl-] detaches; the session stays on the host. `lolidesk <host>` on a host you haven't paired with asks it for consent, as `pair` does; `--token <host token>` connects as its owner instead. `-y` trusts a new host's fingerprint without asking. Remembered hosts are kept in `lolidesk/hosts` under the system config directory (`~/Library/Application Support` on macOS, `~/.config` on Linux), mode 600, shared by the app and the command line.

### Computer use for agents

Any agent on any machine in the network can see and drive a host's screen, through the same host process and the same macOS permissions as the remote desktop: whoever granted Screen Recording and Accessibility for remote control has granted them for agents too.

```sh
claude mcp add desk-studio -- lolidesk mcp 10.188.16.10      # an MCP server over stdio, for Claude Code, Codex …
lolidesk cua 10.188.16.10 screenshot                         # one call from a shell; the image goes to ./screenshot.jpg
lolidesk cua 10.188.16.10 click '{"x": 640, "y": 412}'
lolidesk cua 10.188.16.10 key '{"keys": "cmd+shift+t"}'
```

The host must be remembered first (`lolidesk <host> --token …` once), or pass `--token` and `-y`: the MCP server never prompts.

| Tool | |
| :-- | :-- |
| `screenshot` | JPEG of a display, longest side 1440 px by default (`max_edge`, `0` for native), quality 70. The cursor is in the picture |
| `click`, `move`, `drag`, `scroll` | At pixel coordinates of the last screenshot of that display (native pixels before the first). `click` takes `button`, `count` (double-click is 2) and `modifiers`; `drag` moves in eight steps so apps see the motion; `scroll` takes a direction and a number of wheel notches |
| `type_text` | Unicode text, independent of the keyboard layout and input method |
| `key` | A key or shortcut: `enter`, `tab`, `cmd+c`, `cmd+shift+t`, `pagedown`, `f5`, or any W3C `KeyboardEvent.code` |
| `cursor`, `displays` | Where the pointer is; the displays with their native size |

**A person comes first.** Input from an agent is refused with `USER_ACTIVE` while someone has used the computer in the last 3 seconds: the person at it (from the system's HID idle time, minus the input lolidesk injected itself) or a person in a remote desktop session. Screenshots are always allowed. This follows Cindy's computer-use driver, and keeps an agent from fighting whoever is at the keyboard.

Every tool call is one `CuaReq` on a computer-use stream; an input call is a list of the same `DeskInput` events the remote desktop sends, and the host waits until the injection thread has done them all before answering, so a screenshot taken right after shows the result. Tool names and descriptions are in English, since models read them; the host's own error messages pass through unchanged.

## Protocol

Defined in `desk/proto/src/lib.rs`. Protocol version 2, ALPN `lolidesk/1`, TLS server name `lolidesk`. The ALPN stays the same across versions and `Hello` / `Denied` never change their encoding, so a client and host of different versions get a readable "protocol version mismatch" instead of a failed handshake.

- **Streams.** The first byte of every bidirectional stream is its kind: `0` control, `1` terminal, `2` desktop, `3` computer use. After that come messages framed as `u32 LE length + postcard`, at most 16 MiB each.
- **Control.** `Hello{version, token, client}` → `Welcome{host, os, version, desktop, persistent, allow{terminal, desktop, control, cua, pair, share}, role: Owner|Device(id)}` or `Denied`. `token` is the host token (owner) or a device credential. Before `Hello`, `Pair{device{name, os, kind, version}, code?}` waits for consent (up to 120 s) or redeems a code, and answers `Paired{credential, host, grant}` or `Denied`; the client then sends `Hello` with the credential. After `Hello`: `Access` → `Access{owner, me, grants, codes, pending}`, `CreateCode{label, expires_in?, max_uses?, allow?}` → `Code{info, secret, addr}`, `Revoke(id)`, and for the owner `Decide{id, allow}` and `Limit{grant, allow?}`. Then `ListDisplays` (id, pixels, refresh rate), `ListSessions` (id, title, command, foreground program, working directory, size, viewers, start time, restored, exit status) and `ListLaunchers` (id, name, command). `Permissions` → `Permissions{screen_recording, accessibility}`, what the host process has now, checked without prompting; a host without a remote desktop answers `Error`. `OpenPermission(ScreenRecording|Accessibility)` shows the system prompt on the host and opens that pane of System Settings → `Ok` or `Error`. `RestartHost` → `Ok`, after which the host restarts its LaunchAgent (`launchctl kickstart -k`, from a detached child a second later); a host not running as the LaunchAgent answers `Error`.
- **Terminal.** The first message is `Open{size, command?, cwd?}` or `Attach{id, size}`, where a size is columns, rows and a cell's width and height in pixels. The host answers `Opened{id}`, `Resized{cols, rows}` (the session's size) and a first `Frame` that starts the client's terminal over. Then come `Frame`s (Mirror output), `Resized` between the frames of the old size and those of the new, `Bell`, `Notify{title, body}` and `Exited(code)`. The client sends `Input(bytes)` (already encoded for the terminal's modes), `Resize(size)` (what its view fits), `Claim` (the user is here: take my size), `Clear(Screen|Scrollback)` and `Kill`. Typing also claims. A client whose queue backs up is dropped; reattaching starts it over. `cwd` may start with `~/` or be relative to the home directory.
- **Desktop.** `Start{display, video{long_edge, fps, quality}}` → `Ready{width, height, fps, bitrate}`, the settings the host actually used (no upscaling, frame rate capped at the refresh rate). `Configure(video)` restarts capture with new settings and answers with a new `Ready`. Then `Keyframe`, `Input` and `Stop`. Input coordinates are normalised to 0..1 of the picture. `Key{code, down, mods}` carries a physical key by its W3C `KeyboardEvent.code` name; modifiers are pressed and released as keys of their own, and `mods` lets the host correct what it thinks is held. `Scroll{dx, dy}` is a fraction of the picture's height, positive when the content moves down or right. `Text` injects composed text as Unicode; `ReleaseAll` lets go of everything when the client loses focus.
- **Computer use.** One request, one answer. `Screenshot{display, max_edge, quality}` → `Screenshot{jpeg, width, height, native_width, native_height}`. `Input{display, events, gap_ms}` (the remote desktop's `DeskInput`s, coordinates 0..1, at most 256) → `Done` once they are injected, everything held is released, or `UserActive{idle_ms}`. `Cursor` → `Cursor{display, x, y}`. Any of them can get `Denied` (the host has `cua` off) or `Error`.
- **Datagrams.** `D5 | frame u32 | index u16 | count u16 | flags u8` (little-endian) followed by a fragment. Joined, the fragments are a `VideoFrame`: `key u8 [sps_len u16 sps pps_len u16 pps] avcc`. AVCC is NAL units with 4-byte big-endian length prefixes. A frame that doesn't fit the host's datagram send buffer is dropped whole, the next ones are skipped until a keyframe, and the encoder is asked for one.

## Distribution

lolidesk ships as its own package and is not built into `lolinet`. `lolinet` stays a small static binary (on Linux it is statically linked against musl, so it runs as is on routers and in containers). lolidesk links ScreenCaptureKit's Swift bridge and the system Swift runtime, and the app is a GUI bundle, so they can't follow those rules. Merging them would break both.

| | |
| :-- | :-- |
| **Package** | `lolidesk-<macos\|linux>-<aarch64\|x86_64>.tar.gz`, with `lolidesk`, `libghostty-vt.dylib` (`.so`) and `BUILD-INFO` at the root, and `Lolidesk.app` too in the macOS packages, with its own copy of both in `Contents/Helpers` and `Contents/Frameworks`: the same format as the lolinet package. The Linux packages add `lolidesk-app`, `lolidesk.desktop` and `lolidesk.svg` (from `docs/assets/logo.svg`) at the root, put there by `desk/scripts/bundle-linux.sh`; `install.sh` and `lolinet upgrade` take only `lolidesk` and the library, so they don't install the app. The app also carries mihomo (`Contents/Helpers/mihomo`, the release pinned in `desk/mihomo/mihomo.json`, unmodified, GPL-3.0, with a notice naming the version and its source), which `lolinet mihomo on` runs as a separate program; nothing in lolidesk links it |
| **Version** | `lolidesk --version` prints the version set in `LOLIDESK_VERSION` at build time. CI sets it to `git describe --always` (the short commit hash), so it matches the first line of `BUILD-INFO` and the manifest's `version`; `lolinet upgrade` compares against it. A local build without it prints the Cargo version |
| **libghostty-vt** | Pinned by Ghostty commit and SHA-256 in `desk/ghostty/libghostty-vt.json`; `desk/scripts/fetch-ghostty.sh <platform> <dir>` downloads and checks it. The builds are the ones mygo publishes (macOS 13+; Linux with glibc 2.28+) |
| **CI** | [`desk.yml`](../.github/workflows/desk.yml) builds the same four platforms as lolinet on pull requests and on `main`, all with the app: macOS arm64 / x86_64 and Linux x86_64 / arm64 (arm64 builds natively on GitHub's `ubuntu-24.04-arm` runner; Linux installs GPUI's system packages first and checks that `ldd` finds every library `lolidesk-app` needs). The app's snapshot tests run on both systems; on Linux they draw with Mesa's software Vulkan (lavapipe) and the PNGs are uploaded as an artifact. It runs terminal sessions over loopback, including one that has to come back with its history after a host restart |
| **Release** | Automatic: every push to `main` that changes `desk/` (other than `.md` files) publishes a `lolidesk-<version>` GitHub Release once all four builds pass, with the packages, their `.sha256`, `SHA256SUMS` and the manifest `lolidesk.json`. A tag that already exists is skipped, and running `desk` on `main` with *release* ticked publishes again by hand. The release never takes `latest`, because lolinet's auto-upgrade reads `releases/latest` |
| **Manifest** | `lolidesk.json`, written by `lolinet/release.py --lolidesk`: the same shape as `lolinet.json`, keyed `lolidesk`. See [Release artifacts](reference.md#release-artifacts) |
| **Join site** | The [`dist`](../.github/workflows/dist.yml) workflow copies each release to the join site's `dl/`, packages first and `lolidesk.json` last, when the repository has the Cloudflare secrets ([Self-hosting](self-hosting.md#7--upgrade-devices)). By hand: `uv run join/deploy.py lolidesk <dir>`, with the release's files in `<dir>` |
| **Install** | `install.sh` puts `lolidesk` and libghostty-vt in `/usr/local/lib/lolidesk/` and links `/usr/local/bin/lolidesk` to it, both when it pairs a new device and when it upgrades one (`sudo sh` without a code). An older install's `/usr/local/bin/lolidesk-host` becomes a link too: called by that name the program is `lolidesk host`, so an existing LaunchAgent keeps working. It reads `lolidesk.json` from where it got `lolinet`: on GitHub the newest `lolidesk-*` release, found through the GitHub API because these releases don't take `latest`, else the join site's `dl/`. It checks the sha256 and runs the new `lolidesk --version`, which must print the manifest's version. Any problem skips lolidesk and doesn't affect joining. The host service isn't started |
| **Updates** | `lolinet upgrade`, every 6 hours while auto-upgrade is on, replaces `/usr/local/lib/lolidesk/` when the installed `lolidesk --version` differs from `lolidesk.json` (or when `/usr/local/bin` still holds the old separate binaries). It reads the manifest from the same source as `lolinet.json`: from the same directory for the join site or a manifest URL, from the newest `lolidesk-*` release for a `github:` source. The checks are the same as for `lolinet`: sha256, then a test run that must print the manifest's version. It never installs lolidesk where it isn't, and it never restarts a running host, because terminal sessions live in the host process. A running host keeps the old version until it next starts |
| **Build on macOS** | Xcode 26 or later (macOS 26 SDK) for the host: the capture library's Metal bridge uses types from the new SDK. `make desk-app` builds the app and bundles it with `desk/scripts/bundle-app.sh` into `desk/app-out/Lolidesk.app`. GPUI comes from the `gpui-pre` snapshot that `gpui-kit` pins; a first build takes a few minutes |
| **Build on Linux** | GPUI needs `pkg-config libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libxcb1-dev libx11-xcb-dev libfontconfig-dev libfreetype-dev libvulkan-dev` (Debian / Ubuntu names). `make desk-app` builds the package layout into `desk/app-out/lolidesk/`. The app links those libraries dynamically and, built on Ubuntu 24.04 as in CI, needs glibc 2.39, like the `lolidesk` next to it |

**macOS permissions after an update.** macOS ties Screen Recording and Accessibility to the binary they were granted to. An updated `lolidesk` is a different binary, and on Apple silicon, where it is ad-hoc signed and identified by its hash, macOS may stop honoring the earlier grants. If the desktop then reports that Screen Recording is missing, or input stops working, remove `lolidesk` from both lists in System Settings → Privacy & Security, add it again, and restart the host (`lolidesk host uninstall && lolidesk host install`). `sudo lolinet up --auto-upgrade off` keeps a machine on its current versions, `lolinet`'s included.

## Roadmap

- Capture on Linux / Windows hosts (PipeWire / DXGI) and client decoding off macOS. On anything but macOS, both sides are terminal only for now. For the Linux app: H.264 decoding (VA-API, or software with OpenH264) into RGBA drawn by GPUI, then a Windows build of the app.
- Remote clipboard sync, file transfer, several displays at once, an application menu, remote unlock.
- Adaptive bitrate: today the bitrate follows the chosen quality, and congestion only drops frames and asks for a keyframe. FEC.
- A signed `lolidesk` and `Lolidesk.app` (Developer ID), so macOS keeps its permissions across updates.
- Resuming agents' conversations after a host restart (Claude Code, Codex and others report their session id through a SessionStart hook, as kero does) and showing each session's status (working, waiting for you, idle) in the sidebar.
- Kitty graphics in Mirror frames.
