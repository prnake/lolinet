# Design: one network for servers, desktops and phones

Where lolinet is going, and why each piece is shaped the way it is. What already works is in [Architecture](architecture.md) and the [reference](reference.md); this page also covers what is still being built, and marks it.

The goal is one piece of software for a private network you run yourself: every machine joins the same overlay, any machine can be reached for a terminal, a desktop or an agent, any machine can lend its internet connection, and a phone gets all of this without giving up its own proxy rules.

## Two kinds of machines (and phones)

| | Server | Desktop | Phone |
| :-- | :-- | :-- | :-- |
| **Install** | `curl <join>/i \| sudo sh -s <code>` | `Lolidesk.app`, then the same pairing code in its *Join* page | A mihomo client (Clash Meta for Android, FlClash, Stash …) and a config from a pairing code |
| **Runs** | `lolinet` only: EasyTier, the probe, auto-upgrade, optionally an exit node and the lolidesk host | The same, plus the app: remote terminal and desktop, a computer-use endpoint for agents, invitations, optionally mihomo | mihomo, with a WireGuard outbound into the overlay |
| **Joins with** | TUN (root), or userspace without TUN | TUN, or userspace when mihomo owns the TUN | WireGuard to a relay's VPN portal |
| **GUI** | None | Yes | The mihomo client's |

The server is the base. Everything a desktop has on top lives in the app and in `lolidesk`, so `lolinet` stays one small static binary that runs on routers and in containers (see the size limits in [Architecture](architecture.md#the-lolinet-binary)).

## Joining without TUN

`lolinet up --userspace` (**built**) runs the same EasyTier node with `no_tun = true`:

- **Outbound.** Programs on the machine reach the overlay through a SOCKS5 proxy on `127.0.0.1:18080`, which EasyTier runs on its own user-space TCP/IP stack. Nothing changes in the routing table, so it works in containers, on a NAS without `/dev/net/tun`, and next to another VPN or mihomo that already owns the TUN.
- **Inbound.** Packets to the node's overlay address are delivered to `127.0.0.1` on the same port, so `ssh`, the lolidesk host and the exit node are reachable at the overlay address like on a TUN node. This means every loopback service on a userspace node is reachable from the overlay. Bind services you don't want to share to another address, or keep the node in TUN mode.
- **Same everything else.** The credential, the probe, auto-upgrade and `lolinet status` don't depend on the mode. `lolinet up --tun` switches back.

## Exit nodes

Any node can lend its internet connection to the rest of the network: `sudo lolinet exit on` (**built**).

- **What it is.** A SOCKS5 server built into `lolinet`, with `CONNECT` and `UDP ASSOCIATE`, on port `10180`. On a TUN node it listens on the overlay address only. On a userspace node it listens on `127.0.0.1`, which the overlay reaches through the inbound rule above.
- **Who may use it.** Only overlay addresses (the network's CIDR) and loopback. The public internet can't reach it: the listening address isn't public, and a connection from any other source is closed before the handshake.
- **Why SOCKS5 and not EasyTier's exit node.** EasyTier's own `exit_nodes` sends a TUN client's whole default route through one peer: all or nothing, and only for TUN clients. A SOCKS5 endpoint per node lets a rule engine (mihomo, a browser extension, `curl --socks5-hostname`) pick an exit per domain or per app, works for userspace nodes and phones, and needs no IP forwarding or NAT on the exit node.
- **Speed.** Each TCP connection is two sockets spliced with `copy_bidirectional` and `TCP_NODELAY` on both sides. UDP is relayed packet by packet with one socket per association, so QUIC and DNS work.
- **Service.** `co.lo-li.lolinet-exit` (macOS) or `lolinet-exit.service` (Linux), running as `nobody`. Settings in `/etc/lolinet/exit.json`.
- **Discovery** (**built**). `lolinet exit on` registers the node with the manager (and `exit off` takes it off), which lists exit nodes for mihomo on desktops and phones.

## Invitations and addresses

Today pairing codes come from an admin with the console password (`scripts/pair.py`, or the manager's *Pair* page), and the admin picks the overlay address.

The goal: **any member can invite**, and **the central server picks the address** (**built**).

- **`lolinet invite [--name N] [--kind desktop|userspace|phone|server]`** asks the manager on the issuer relay, over the overlay, for a pairing code, and prints the `curl … | sudo sh -s CODE` line, or for a phone the `<join>/m#CODE` link. The app's *Invite* button (**built**) does the same, with a QR code for the phone link.
- **Addresses** come from the pool for the kind, from the same address plan as the node list (`10.188.1.0/24` servers, `10.188.16.0/20` desktops, `10.188.32.0/20` userspace, `10.188.48.0/20` phones). The manager skips every address in its device registry and in the live route table.
- **Credentials** for member invitations are signed by the issuer relay itself, through its local EasyTier RPC, so no console session is needed. They are single-use and expire like admin-issued ones.
- **Who is asking.** The request arrives over the overlay; its source address is the member's overlay address, and the manager checks that the route table has a peer with that address. Overlay addresses aren't cryptographically bound to credentials yet, so this is a convenience check, not a security boundary. Signing requests with the node's credential key is planned.
- **Permissions.** `policy.json` in the manager's state decides who may invite which kinds, and how many codes per day (default: every member may invite desktops, userspace nodes and phones, 20 per day; only admins may invite servers). Every invitation is written to the audit log with the inviter.

## The desktop app

The desktop app is the desktop side of everything: `Lolidesk.app` on macOS, and `lolidesk-app` in the Linux package (**built**: what is built below works the same there, with administrator prompts through `pkexec`; showing the remote desktop on Linux needs a video decoder and is **planned**). Windows later.

- **Join** (**built**): paste the pairing command or enter the join site and code; the app checks both, runs the join script with an administrator prompt and shows its output. **Invite** (**built**): make a code for another machine or a phone (with a QR code).
- **Network**: this node, its peers and their latency, the exit node switch and the mihomo switch (**built**); a list of the network's exit nodes and a switch for TUN or userspace (**planned**).
- **Hosts**: remote terminals and desktops, as today, with this computer first (**built**): the app connects to its own host as the owner, and starts the host service once if it isn't running. Below the hosts, the network's other devices that run a lolidesk host, each with *Connect* (**built**; see below).
- **Shell**: `lolinet` comes with joining; the app shows whether `lolinet` and `lolidesk` are in `/usr/local/bin` and links its own `lolidesk` there with one button (**built**), so everything the app does is also a command. On Linux a second button adds the app to the application menu (**built**). Doing it on first run is **planned**.
- **Permissions guide** (**built**): Screen Recording and Accessibility, checked live, each with a button that opens the right settings page on the host, and a button that restarts the host service once Screen Recording is on.

## Remote control and computer use, one permission

A machine's lolidesk host already holds the two macOS permissions that remote control needs: Screen Recording to capture, Accessibility to inject input. An agent driving the machine needs exactly the same two. So the host exposes both through one grant (**built**):

- **Computer-use tools** over the same QUIC connection as the desktop and the terminal, authenticated by the same token and pinned certificate: `screenshot` (JPEG, scaled to a size the agent asks for), `click` (with button, count and modifiers), `move`, `drag`, `scroll`, `type_text`, `key` (`cmd+shift+t` and the like), `displays`, `cursor`. Coordinates are the last screenshot's pixels.
- **`lolidesk mcp <host>`** is a stdio MCP server that forwards these tools to a host, so any agent on any machine in the network can use any other machine: `claude mcp add desk-studio -- lolidesk mcp 10.188.16.10`. Without a host it serves this computer, as the owner, with nothing to pair. `lolidesk cua <host> <tool> [json]` calls one tool from a shell.
- **Same permissions, no new prompt.** If the host can capture the screen for a viewer, it can take a screenshot for an agent. Nothing new to grant.
- **Permission switches.** `lolidesk host allow` sets `terminal`, `desktop`, `control` and `cua`, all on by default. A host can serve desktops view-only (`control off`) or keep agents off (`cua off`). Each paired device can be limited further (`lolidesk host limit`, **built**; see below).
- **Person first.** As in Cindy's computer-use driver, input from an agent is refused while a person has used this Mac's keyboard or mouse in the last few seconds (`user_active`), so an agent never fights the person in front of the screen. Remote desktop viewers are people and aren't held back.

## Computers pairing with computers

Joining the network (above) makes a machine a member. Reaching its terminals and screen is a second, per-computer decision, because every computer runs the same program and is a host as well as a client (**built**, in `lolidesk` and in the app):

- **This computer is the first host.** The host also listens on `127.0.0.1`, and the app and command line on the same machine connect as its *owner* with the host's token from the config directory. A terminal opened locally lives in the local host, so anyone you let in sees it too. The app shows it as *本机* above the other hosts and, opened from its bundle, installs the host service (a LaunchAgent on macOS, a `systemd --user` unit on Linux) when none is installed; one switch turns that off. An installed service is never moved to the app's own binary, because macOS ties Screen Recording and Accessibility to the program they were granted to.
- **Connect, then consent.** The app lists the devices in the network that run a lolidesk host. Any of them can ask to pair with *Connect*; the owner sees *「laptop」(10.188.16.11, macOS) wants to connect* over the app's window, with a system notification when the window is in the background, or a system dialog when the app isn't open, and agrees once. The device then gets its own credential and never asks again.
- **Codes and share links, one to many.** The owner can make pairing codes (optionally expiring, limited in uses, or view-only), and a paired device can make share links for the same host to bring in its other machines. A link carries the host's address and certificate fingerprint, so importing it needs no checking by eye. The app shows a new code with its link and a QR code, imports links, and opens `lolidesk://` links clicked elsewhere.
- **One place to see and revoke.** The host lists every device grouped by how it paired: by consent, or under the code it used, with its system and last connection. Revoking a code removes its devices; revoking a device removes its share links and the devices they brought in, and drops their open connections immediately. In the app this is the *Access* page under *本机*, which also limits a device with a few switches; on another host it shows your own entry and share links there.
- **Room for finer permissions.** A device may do what the host offers (`lolidesk host allow`) and what its own limit allows (`lolidesk host limit`, or the code it used). Scopes per session or per display would go in the same place.

This replaces one shared token per host, where a host couldn't tell its clients apart and revoking one meant revoking all.

## mihomo on phones and desktops

A phone allows one VPN at a time. If that VPN is lolinet, the phone loses its own proxy rules; if it's mihomo, it can't reach the overlay. The answer is to make the overlay one of mihomo's outbounds.

```
phone ─ mihomo (the only VPN)
          ├─ 10.188.0.0/16, *.local names ──▶ WireGuard ──▶ relay's VPN portal ──▶ overlay
          ├─ rules: some sites ──▶ socks5 10.188.x.y:10180 (an exit node), dialed through the WireGuard outbound
          ├─ ads, trackers ──▶ REJECT
          └─ everything else ──▶ DIRECT (or the phone's own proxies)
```

- **Into the overlay** (**built**). EasyTier v2.7's VPN portal on the issuer relay accepts named WireGuard clients, each with its own key (derived from the portal's key and the name) and overlay address, and attaches each as a peer. `lolinet invite --kind phone` adds one through the relay's local RPC; the phone opens `<join>/m#CODE`, and the page decrypts the mihomo config in the browser with the same parameters as `install.sh`, so the code never reaches a server. mihomo has a built-in user-space WireGuard outbound, so no second VPN and no EasyTier on the phone. Not yet tested against a real mihomo client: the config is checked for every field and parsed as YAML, but no WireGuard handshake has been made with it.
- **Exits.** Exit nodes are ordinary `socks5` proxies in mihomo with `dialer-proxy` set to the WireGuard outbound, so a rule can send a site out through the office, home or a relay.
- **Routes to a specific machine** (**built**). Besides the general exit group, the manager's **分流** page defines routes: *these rules leave through that machine*, e.g. `DOMAIN-SUFFIX,web.com` (which also matches `*.web.com`) through `office`. Each route is its own rule set and group in mihomo; its rules and its node are fetched live, so moving a route to another node needs nothing on the clients. Adding a route changes the config itself: desktops pick it up within 6 hours (or `sudo lolinet mihomo refresh`), phones re-import. A route whose node is offline fails closed.
- **Rules from the center** (**built**). The pairing code gives the phone a config with its WireGuard keys once. Its `rule-providers` and `proxy-providers` point at the manager, fetched through the WireGuard outbound (`proxy: lolinet`), so the admin changes routing for every phone in one place, and nothing about the network is ever on a public URL.
- **Desktops.** Two layouts, both generated by `lolinet mihomo`, and `sudo lolinet mihomo on` runs it as a service (**built**): lolinet in TUN mode with mihomo's TUN excluding `10.188.0.0/16`, or lolinet in userspace mode with mihomo as the only TUN. In both, mihomo's own connections (exit nodes, rule fetches) go through lolinet's SOCKS5 on `127.0.0.1:18080`, because mihomo binds its outgoing sockets to the physical interface and couldn't reach lolinet's TUN otherwise; details in [Architecture](architecture.md#phones-and-mihomo). Not yet run against a real mihomo. The app ships mihomo (**built**, pinned in `desk/mihomo/mihomo.json`) and `lolinet mihomo on` finds it there; `lolinet` never contains it, to stay small. The app's network page has a switch for it (**built**). The service runs a root-owned copy of mihomo, never the app's own file, which the user can write.

## What it borrows

| From | What |
| :-- | :-- |
| Tailscale | Exit nodes, and `tailscale serve`-style "every node is reachable at its address" in userspace mode |
| Headscale, NetBird | The center assigns addresses; members invite members within a policy |
| Cindy | One permission grant for remote control and computer use; refuse agent input while the person is active |
| Tailscale, Vibe42 | A request the other computer approves once, then a per-device key you can see and revoke; share links that bring in more devices |
| Vibe42, kero | Terminals that live on the host (libghostty-vt Mirror frames), sessions that survive restarts |
| mihomo, Stash | The overlay as one outbound among the user's own rules, instead of a second VPN |
