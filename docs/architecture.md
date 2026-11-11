# Architecture

How lolinet's pieces work and why they are built the way they are. Commands and file paths are listed in the [reference](reference.md).

```
device ──wss :443, your SNI──▶ relay ◀── tcp / udp, every pair ──▶ relay ◀──▶ relay …
  │                              ▲
  │                              └─ issuer relay: easytier-web console, HTTPS on the overlay only
  │
  └─ curl https://<join site>/i | sudo sh -s <pairing code>
       └─ join site (Cloudflare Worker)
            KV: one-time pairing ciphertext · R2: install.sh, packages, manifest, probe page · Durable Object: probe state
```

The network itself is plain [EasyTier](https://github.com/EasyTier/EasyTier) v2.7.0 with three patches. Everything else in this repository sits around it: the `lolinet` binary that runs it on devices, the scripts that sign credentials and issue pairing codes, the join site that hands them out, and the console and probe that show the result.

## The network

**Relays** are EasyTier nodes on hosts with public addresses. They run in:

- **Secure mode.** Nodes join with a credential signed by the console instead of a shared network secret.
- **Private mode.** Relays don't forward traffic for networks other than yours.

Every relay connects to every other relay over TCP and UDP, so any two relays are one hop apart.

**The console** is EasyTier's `easytier-web-embed` on the issuer relay (`network.issuer`), with the network in Gateway mode. It listens only on the overlay, behind HTTPS on the issuer's overlay address. Scripts connect straight to that IP and use the console's name only for SNI and certificate checks, so a resolver that hands out fake IPs doesn't get in the way.

**Devices** get a config rendered by [`scripts/device_config.py`](../scripts/device_config.py):

| Setting | Why |
| :-- | :-- |
| `[secure_mode]` with the device's own `local_private_key` | The credential. There is no `network_secret` in the file |
| `[[peer]] uri = "wss://<relay ip>:443/?sni=<network.sni>"` | Port 443 and a TLS server name you choose get through networks that only allow ordinary HTTPS. `--transport tcp` uses `tcp://<ip>:<et_port>` instead, for stock EasyTier |
| `private_mode = true` | Same as the relays |
| `latency_first = true` | EasyTier's default route takes the fewest hops: a direct path wins even when it's slow. With this on, a device follows the network-wide latency table and goes through the nearest relay when that is faster. Relays keep the default |
| `enable_kcp_proxy = true` | TCP flows travel inside KCP over UDP, so ISP shaping of long cross-border TCP flows doesn't apply |

Peer URLs use the relay's public IP, not its DNS name: on some networks the local resolver answers with fake IPs (`198.18.x.x`), and the SNI comes from `?sni=` either way. `--by-name` switches to `<relay>.<edge_zone>`.

## The patches

[`easytier/build.env`](../easytier/build.env) pins the upstream tag and commit (CI checks that the tag still points there) and lists the patches in order.

- **[`easytier-v2.7.0-wss-sni.patch`](../patches/easytier-v2.7.0-wss-sni.patch)**: upstream EasyTier sends the host from the URL as SNI, and for an IP address that is `localhost`. The patch takes the SNI from the URL's `sni` query parameter, or from `ET_WSS_SNI` when the URL has none (peer URLs learned from other nodes carry no query), and strips the parameter from the WebSocket request.
- **[`easytier-v2.7.0-udp-tunnel-pref.patch`](../patches/easytier-v2.7.0-udp-tunnel-pref.patch)**: upstream EasyTier keeps several connections to a peer but sends everything over one default connection, re-picked every 5 s by latency. So a QUIC stream (lolidesk) can end up inside a TCP or WSS tunnel and stall behind every retransmit, and a KCP stream can end up inside a UDP tunnel, where it collapses under loss. The patch sets a per-send preference. The sending node looks at the inner packet before encrypting it: UDP prefers a UDP tunnel, and KCP or QUIC proxy packets prefer TCP or WSS. A relay can't see the inner packet, so it sends KCP and QUIC packets (their type is in the header) over TCP or WSS, and forwards other packets over UDP when they arrived over UDP. Nothing on the wire changes, and when no connection of the preferred kind exists the old default connection is used. The UDP hole puncher also keeps trying when the only direct connection is TCP. Relays need both `tcp://` and `udp://` peers to each other, and devices a `udp://` peer next to each `wss://` one, for this to matter.
- **[`easytier-v2.7.0-lolinet.patch`](../patches/easytier-v2.7.0-lolinet.patch)**: adds `lolinet` as a third binary of the `easytier` crate and makes `easytier-cli`'s `main` public, so `lolinet` can call it.

[`easytier/check_sni.py`](../easytier/check_sni.py) proves the first patch works. It opens a TCP port as a fake relay, lets `easytier-core` connect over WSS, parses the TLS ClientHello it sends, and checks the SNI, both with `?sni=` and with `ET_WSS_SNI`. It doesn't look for strings in the binary: on x86_64, short constants end up as instruction immediates and can't be found. CI runs it on every build that its runner can execute, against both `easytier-core` and `lolinet` linked under that name.

[`easytier/check_udp_pref.py`](../easytier/check_udp_pref.py) proves the tunnel preference. It starts three nodes on loopback (device, relay, device), each pair joined over both TCP and UDP, and counts the bytes each connection sends on both hops. In one scenario the UDP links are 20 ms slower, so the default is TCP: UDP traffic must still take UDP. In the other the TCP links are slower and KCP is on: KCP traffic must still take TCP. Run with `--expect-baseline` against an unpatched build, it checks that both cases follow the default connection instead, so the test can tell the two apart. CI runs it against `lolinet`.

## The `lolinet` binary

**One binary, dispatched on `argv[0]`.** Called `easytier-core`, it runs `easytier::core::main`, EasyTier's own entry point. Called `easytier-cli`, it runs the CLI, compiled in as a module from `easytier-cli.rs`. Called `lolinet`, it runs its own commands. `lolinet run`, `peers`, `core` and `cli` re-exec the binary under the right name and pass the arguments through unchanged.

**Built inside EasyTier.** The source lives in [`lolinet/src`](../lolinet/src) and is copied into `easytier/src/lolinet/` at build time. It shares `easytier-core`'s dependencies and `Cargo.lock` (`--locked`). Linux builds are static musl binaries, so they also run on routers and in containers. The aarch64 and Apple-silicon builds use mimalloc, the others jemalloc, the same as upstream's CI.

**Kept small on purpose.** CI fails a `lolinet` binary over 32 MiB or a package over 14 MiB, and fails a Linux build that isn't statically linked. Anything with a GUI or system-framework dependencies, such as [lolidesk](lolidesk.md), ships as a separate package.

**The service.** `lolinet up` copies itself to `/usr/local/bin/lolinet` (write to a temp file, then rename, so a running old version isn't disturbed), puts the config at `/etc/lolinet/loli.toml` with mode 600, and registers the LaunchDaemon `co.lo-li.lolinet` (macOS) or `lolinet.service` (Linux). Both run `lolinet run`, which starts the kernel with `--rpc-portal 127.0.0.1:15888`: EasyTier v2.7.0 ignores `rpc_portal` in the config file and would otherwise listen on `0.0.0.0`. The kernel logs only to `/var/log/lolinet/easytier.log`, 10 MB × 5. Its terminal log is off, because hole-punching warnings are frequent and launchd doesn't rotate stderr; argument errors and panics still reach launchd or journald.

**Taking over older installs.** Earlier versions of the join script installed `easytier-core` and `easytier-cli` as a service named `easytier`. `lolinet up`:

- removes that service, which would compete for the same TUN device and credential;
- copies `/etc/easytier/loli.toml` to `/etc/lolinet/loli.toml` if the new one doesn't exist yet, and leaves the old file in place;
- replaces `easytier-core` and `easytier-cli` in the old directory (`/usr/local/libexec/easytier/` on macOS, `/opt/easytier/` on Linux) with symlinks to `lolinet`, so no-tun clients, a not-yet-migrated Python probe and older scripts keep working.
- moves a device-mode Python probe (`/etc/mesh-probe/reporter.json`) to the built-in one, keeping its public id.

**Credential expiry.** `pair.py` appends `# credential_id` and `# credential_expiry_unix` to the config as TOML comments. EasyTier ignores them; `lolinet status` reads them to show the days left. The config is mode 600, so this line needs `sudo`.

## TUN and userspace

A node joins in one of two modes. Both use the same credential, probe and auto-upgrade; only how traffic enters and leaves the overlay differs.

| | TUN (default) | Userspace (`lolinet up --userspace`) |
| :-- | :-- | :-- |
| **Outbound** | A TUN interface takes the overlay's CIDR; any program reaches overlay addresses directly | Programs use the SOCKS5 proxy on `127.0.0.1:18080`, which EasyTier serves from its own user-space TCP/IP stack (smoltcp). The routing table isn't touched |
| **Inbound** | Packets for the node's overlay address arrive on the TUN interface | EasyTier delivers TCP and UDP for the node's overlay address to `127.0.0.1`, same port, through its proxy |
| **Needs** | Root and `/dev/net/tun` | Nothing from the kernel. Works in containers and next to another VPN or mihomo that owns the TUN |

The mode is two lines of `loli.toml`: `socks5_proxy = "socks5://127.0.0.1:18080"` at the top and `no_tun = true` under `[flags]`. `lolinet up --userspace` and `--tun` edit those lines in place rather than rewriting the file, because `# credential_id` and other comments at the end are read by `lolinet status`, and check the result with `easytier-core --check-config` before it replaces the old config.

Inbound delivery to loopback has a consequence worth knowing: on a userspace node, every service that listens on `127.0.0.1` is reachable from the overlay at the node's address. Keep private services on another address, or keep the node in TUN mode.

## Exit nodes

`lolinet exit on` makes a node lend its internet connection to the rest of the network through a SOCKS5 server built into `lolinet` ([`lolinet/src/exit.rs`](../lolinet/src/exit.rs)). A rule engine on the client side (mihomo on a phone, a browser's proxy settings, `curl --socks5-hostname`) decides what goes out where; nothing else changes on the exit node: no IP forwarding, no NAT rules.

- **Listening.** On the overlay address in TUN mode. On `127.0.0.1` in userspace mode, where EasyTier's inbound delivery brings overlay connections to it. In TUN mode the service waits for the address to appear, since it may start before EasyTier has brought up the interface.
- **Who may connect.** The source must be in `allow` (default: the overlay's CIDR, plus loopback). Any other source is closed before the SOCKS5 handshake. There is no SOCKS5 authentication: the overlay already authenticated the peer.
- **Where it goes.** Loopback, link-local (including cloud metadata at `169.254.169.254`), multicast and unspecified destinations are always refused, so the exit can't be used against the exit node itself. Private and CGNAT ranges (`10/8`, `172.16/12`, `192.168/16`, `100.64/10`, `fc00::/7`) are refused unless the node runs with `--lan`, which makes it a subnet router for its local network. Domain names are resolved on the exit node and the resulting address is checked the same way.
- **TCP** is two sockets with `TCP_NODELAY`, copied in both directions with 64 KiB buffers; on loopback it moves about 30 Gbit/s, as fast as the test client. **UDP** (`UDP ASSOCIATE`) gives each association its own relay socket on the listening address and a pair of outbound sockets (IPv4 and IPv6). The association ends when its TCP control connection does, as RFC 1928 requires. In userspace mode the relay socket is on loopback, so the reply names `0.0.0.0` and clients use the address they connected to, which is the convention.
- **The service** starts as root to read `loli.toml` (mode 600) and bind, then drops to `nobody`. It runs on one thread, caps itself at 4096 connections, and logs only errors.

[`lolinet/check_exit.py`](../lolinet/check_exit.py) tests all of this without root: the refusals, `CONNECT` to the internet, DNS over `UDP ASSOCIATE`, and two userspace nodes on loopback where one reaches the other's exit through its own SOCKS5 and the other's overlay address. CI runs it on every platform that can execute the build.

## Pairing

```
admin                         console (issuer)        join site (Worker + KV)         device
  │ pair.py new ─────────────▶ new credential
  │ render config, encrypt with a fresh code
  │ PUT pair:<sha256(code)[:32]> = ciphertext, TTL 15 min ─────▶
  │ shows: curl …/i | sudo sh -s CODE
  │                                                                   ◀── GET /i
  │                                                                       download lolinet (GitHub, else dl/),
  │                                                                       check sha256, test-run it
  │                                                                   ◀── GET /v1/pair/<sha256(code)[:32]>
  │                                                  ciphertext, then delete ──▶ openssl decrypt with CODE
  │                                                                       lolinet up -c loli.toml
```

- **Code**: 12 characters from an alphabet without look-alikes (no `0`, `O`, `1`, `I`, `L`), shown in groups of four.
- **Encryption**: PBKDF2-SHA256 with 200,000 rounds derives the key and IV, then AES-256-CBC, in OpenSSL's `Salted__` format. The device decrypts with `openssl enc -d -aes-256-cbc -pbkdf2 -iter 200000 -md sha256`, which every macOS and Linux has.
- **The Worker** stores the ciphertext under a hash of the code, deletes it on the first `GET`, and allows each IP 20 attempts per 10 minutes. `HEAD` requests are refused so they can't use up a code. The code itself never reaches the Worker.
- **The probe** rides along: unless the code was made with `--no-probe`, the config ends with a `# probe = {…}` line holding the device's public id and report key. `lolinet up` moves that line to `/etc/lolinet/probe.json` and installs the probe service; it never reaches EasyTier.
- **Before redeeming**, `install.sh` checks that the machine is supported (macOS, or Linux with systemd) and downloads and verifies `lolinet`, so a code isn't spent on a machine that can't use it or can't get the client.
- **Where `lolinet` comes from.** The latest GitHub Release, pinned to one tag through the `releases/latest` redirect. If GitHub is unreachable or too slow (common from mainland China), the join site's `dl/`, which holds the same release's files. `lolinet up` then points auto-upgrade at the source that worked, so a device never flips between two sources that are briefly on different versions.
- **Without a code** the same script upgrades a device already in the network: it reuses the config on the machine and runs `lolinet up` only if the version, the service or the upgrade job needs it.
- **lolidesk** comes last, in both cases: `install.sh` installs or updates `lolidesk` in `/usr/local/lib/lolidesk/` (linked from `/usr/local/bin/lolidesk`) from the same source as `lolinet`, only if it passes the sha256 check and runs on this machine. It doesn't start the host. Any failure here skips lolidesk and leaves the join alone.

### Member invitations

The admin flow above needs the console password. Members can also invite: `lolinet invite` asks the manager (`web/agent/mesh_manager.py`, on the issuer relay behind nginx on the overlay) for a code over the overlay. The manager identifies the member by the request's overlay source address, checks `policy.json`, picks an address from the pool for the requested kind, and has the issuer relay's own EasyTier node sign the credential through its local RPC; the relay keeps such credentials in its `credential_file`. From there it is the same flow: render, encrypt, KV, one-time code. [`lolinet/check_invite.py`](../lolinet/check_invite.py) runs the whole path on loopback (an issuer, a member, a stand-in Worker and nginx) and joins a new node with the code it gets.

## Auto-upgrade

`lolinet up` installs a job that runs `lolinet upgrade --auto` every 6 hours: the LaunchDaemon `co.lo-li.lolinet-upgrade`, or `lolinet-upgrade.timer` with up to 30 minutes of random delay.

**The manifest** (`lolinet.json`, written by [`lolinet/release.py`](../lolinet/release.py)) names the version devices should run. `lolinet` doesn't compare old and new: if the local version differs, it switches. Rolling back means publishing the old version again.

**Where it looks.** The settings in `/etc/lolinet/lolinet.json` decide: an explicit source (`github:<owner>/<repo>` or a URL), else the join site's `<base_url>/dl/lolinet.json`, else GitHub `prnake/lolinet`. `install.sh` sets the explicit source to whichever one it downloaded from. Packages are resolved next to the manifest. GitHub's `releases/latest/download/lolinet.json` first redirects to `releases/download/<tag>/lolinet.json`, which pins the version, and then to a one-time storage URL. `lolinet` follows only redirects that keep the file name, so it builds package URLs from the pinned directory and never from the storage URL.

**Three checks before switching:**

1. Download with the system's `curl`.
2. The sha256 must match the manifest.
3. The new binary must run on this machine and print the manifest's version from `--version`.

Then it replaces `/usr/local/bin/lolinet` atomically and restarts the service. The probe is part of the binary, so it switches with it; each run also tidies the probe (takes over a Python one, reinstalls a service file this version writes differently). The job logs only switches and errors.

**lolidesk** follows its own manifest, `lolidesk.json`, because its releases are separate and never `latest`. From the join site or a manifest URL it is the file next to `lolinet.json`; from a `github:` source it is in the newest `lolidesk-*` release, which `lolinet` finds through the GitHub API. Only binaries already in `/usr/local/bin` are considered, and only when their `--version` differs from the manifest. They pass the same sha256 check and test run, and then the files are replaced. A running host isn't restarted, since that would end its terminal sessions; it picks up the new version on its next start.

## Public probe

The join site's home page shows the network's health to anyone, without naming anything.

**Reports.** Devices report with `lolinet probe`, built into `lolinet` ([`src/probe.rs`](../lolinet/src/probe.rs)): it reads hardware from `system_profiler` / `sysctl` (macOS) or `/proc` and `/sys/class/dmi` (Linux), asks the local kernel for the nearest relay, and sends through the system's curl with the key on stdin. It starts as root and drops to `nobody`. Relays, and devices in Docker that can't run the `lolinet` service, use [`probe/reporter.py`](../probe/reporter.py), which needs only the Python standard library (macOS's `/usr/bin/python3` 3.9 is enough) and runs without root. Both send the same format with the same signature.

- **Relays** report every 30 seconds: CPU, memory, disk, network rate and traffic, load, uptime, OS and EasyTier version from `/proc` and `easytier-cli -o json peer`, plus their links to other relays and to devices.
- **Devices** send a heartbeat every 60 seconds with hardware only: model, chip, memory, total storage, OS and architecture, and a location. The location is a fixed city, or "near <city>" for the nearest relay by measured latency. Never a hostname, address, serial number or load.

**Keys.** Everything derives from one master key, which lives only in `$LOLINET_HOME/state/mesh-probe.json` and in the Worker secret `PROBE_MASTER`:

| Key | Derivation | Held by |
| :-- | :-- | :-- |
| Relay report key | `HMAC-SHA256(master, "probe:<relay name>")` | That relay |
| Device id key | `HMAC-SHA256(master, "devid")` | Relays, to turn the hostnames they see into public ids |
| Device public id | `d-` + first 10 hex of `HMAC-SHA256(devid key, hostname)` | Computed by the relay and by `pair.py` |
| Device report key | `HMAC-SHA256(master, "probe:<public id>")` | That device |

Each report is signed as `X-Probe-Sig = hex(HMAC-SHA256(key, "<timestamp>." + body))`. A compromised relay or device can forge only its own card. The Worker accepts any `d-…` id signed with the right derived key, so adding a device needs no change to the Worker.

**What the page shows.** Relays appear by region code (shown as a city) and spec; relay names are used only to check signatures and to keep history, and never leave the Worker. Devices appear by anonymous id with their hardware, location and which relays they reach directly. The probe page is a separate build of the console's components ([Local console](console.md#public-probe)); a CI check fails if it contains any relay or device name, overlay address, the SNI, the console's host or a secret field name.

**State.** One Durable Object (`ProbeHub`, SQLite) holds the latest report per node, 31 days of heartbeats and 2 days of link latency in 5-minute buckets.

## Redirect pages

`<name>.<edge_zone>` hosts listed in `cloudflare.redirects` are bound to the Worker. The page tries to fetch `https://<name><local_suffix>/.well-known/mesh-ping` from the browser, as a local-network request with a 3-second timeout. If the service answers, the browser is on the overlay and the page redirects there. If not, it says so and offers a direct link. The internal service itself is never exposed publicly.

## Repository and deployment

This repository holds code only. A deployment is the code plus a node list (`nodes.yaml`, or `LOLINET_NODES`) and an admin directory (`$LOLINET_HOME`) that hold everything specific to one network. All packages are built and released by this repository's workflows; the join site's download area is a copy of the latest lolinet and lolidesk releases, uploaded unchanged by the [`dist`](../.github/workflows/dist.yml) workflow (or `join/deploy.py release` and `lolidesk` by hand).
