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

- **`lolinet invite [--name N] [--kind desktop|userspace|phone|server]`** asks the manager on the issuer relay, over the overlay, for a pairing code, and prints the `curl … | sudo sh -s CODE` line, or for a phone the `<join>/m#CODE` link. The app's *Invite* button (**planned**) does the same.
- **Addresses** come from the pool for the kind, from the same address plan as the node list (`10.188.1.0/24` servers, `10.188.16.0/20` desktops, `10.188.32.0/20` userspace, `10.188.48.0/20` phones). The manager skips every address in its device registry and in the live route table.
- **Credentials** for member invitations are signed by the issuer relay itself, through its local EasyTier RPC, so no console session is needed. They are single-use and expire like admin-issued ones.
- **Who is asking.** The request arrives over the overlay; its source address is the member's overlay address, and the manager checks that the route table has a peer with that address. Overlay addresses aren't cryptographically bound to credentials yet, so this is a convenience check, not a security boundary. Signing requests with the node's credential key is planned.
- **Permissions.** `policy.json` in the manager's state decides who may invite which kinds, and how many codes per day (default: every member may invite desktops, userspace nodes and phones, 20 per day; only admins may invite servers). Every invitation is written to the audit log with the inviter.

## The desktop app

`Lolidesk.app` (macOS first, then Linux; Windows later) is the desktop side of everything:

- **Join** (**planned**): enter a pairing code, the app runs the join script with an administrator prompt and shows progress. **Invite**: make a code for another machine.
- **Network** (**planned**): this node, its peers and their latency, exit nodes, and switches for TUN or userspace, exit node and mihomo.
- **Hosts**: remote terminals and desktops, as today.
- **Shell**: on first run the app links `lolinet` and `lolidesk` into `/usr/local/bin`, so everything the app does is also a command.
- **Permissions guide** (**planned**): Screen Recording and Accessibility, checked live, each with a button that opens the right settings page.

## Remote control and computer use, one permission

A machine's lolidesk host already holds the two macOS permissions that remote control needs: Screen Recording to capture, Accessibility to inject input. An agent driving the machine needs exactly the same two. So the host exposes both through one grant (**built**):

- **Computer-use tools** over the same QUIC connection as the desktop and the terminal, authenticated by the same token and pinned certificate: `screenshot` (JPEG, scaled to a size the agent asks for), `click` (with button, count and modifiers), `move`, `drag`, `scroll`, `type_text`, `key` (`cmd+shift+t` and the like), `displays`, `cursor`. Coordinates are the last screenshot's pixels.
- **`lolidesk mcp <host>`** is a stdio MCP server that forwards these tools to a host, so any agent on any machine in the network can use any other machine: `claude mcp add desk-studio -- lolidesk mcp 10.188.16.10`. `lolidesk cua <host> <tool> [json]` calls one tool from a shell.
- **Same permissions, no new prompt.** If the host can capture the screen for a viewer, it can take a screenshot for an agent. Nothing new to grant.
- **Permission switches.** `lolidesk host allow` sets `terminal`, `desktop`, `control` and `cua`, all on by default. A host can serve desktops view-only (`control off`) or keep agents off (`cua off`). Per-token scopes come next (**planned**).
- **Person first.** As in Cindy's computer-use driver, input from an agent is refused while a person has used this Mac's keyboard or mouse in the last few seconds (`user_active`), so an agent never fights the person in front of the screen. Remote desktop viewers are people and aren't held back.

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
- **Rules from the center** (**built**). The pairing code gives the phone a config with its WireGuard keys once. Its `rule-providers` and `proxy-providers` point at the manager, fetched through the WireGuard outbound (`proxy: lolinet`), so the admin changes routing for every phone in one place, and nothing about the network is ever on a public URL.
- **Desktops.** Two layouts, both generated by `lolinet mihomo` (**built**): lolinet in TUN mode with mihomo's TUN excluding `10.188.0.0/16`, or lolinet in userspace mode with mihomo as the only TUN, sending the overlay to lolinet's SOCKS5 on `127.0.0.1:18080`. The app can ship mihomo itself; `lolinet` never links it, to stay small.

## What it borrows

| From | What |
| :-- | :-- |
| Tailscale | Exit nodes, and `tailscale serve`-style "every node is reachable at its address" in userspace mode |
| Headscale, NetBird | The center assigns addresses; members invite members within a policy |
| Cindy | One permission grant for remote control and computer use; refuse agent input while the person is active |
| Vibe42, kero | Terminals that live on the host (libghostty-vt Mirror frames), sessions that survive restarts |
| mihomo, Stash | The overlay as one outbound among the user's own rules, instead of a second VPN |
