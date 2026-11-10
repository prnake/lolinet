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

The network itself is plain [EasyTier](https://github.com/EasyTier/EasyTier) v2.7.0 with two patches. Everything else in this repository sits around it: the `lolinet` binary that runs it on devices, the scripts that sign credentials and issue pairing codes, the join site that hands them out, and the console and probe that show the result.

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
- **[`easytier-v2.7.0-lolinet.patch`](../patches/easytier-v2.7.0-lolinet.patch)**: adds `lolinet` as a third binary of the `easytier` crate and makes `easytier-cli`'s `main` public, so `lolinet` can call it.

[`easytier/check_sni.py`](../easytier/check_sni.py) proves the first patch works. It opens a TCP port as a fake relay, lets `easytier-core` connect over WSS, parses the TLS ClientHello it sends, and checks the SNI, both with `?sni=` and with `ET_WSS_SNI`. It doesn't look for strings in the binary: on x86_64, short constants end up as instruction immediates and can't be found. CI runs it on every build that its runner can execute, against both `easytier-core` and `lolinet` linked under that name.

## The `lolinet` binary

**One binary, dispatched on `argv[0]`.** Called `easytier-core`, it runs `easytier::core::main`, EasyTier's own entry point. Called `easytier-cli`, it runs the CLI, compiled in as a module from `easytier-cli.rs`. Called `lolinet`, it runs its own commands. `lolinet run`, `peers`, `core` and `cli` re-exec the binary under the right name and pass the arguments through unchanged.

**Built inside EasyTier.** The source lives in [`lolinet/src`](../lolinet/src) and is copied into `easytier/src/lolinet/` at build time. It shares `easytier-core`'s dependencies and `Cargo.lock` (`--locked`). Linux builds are static musl binaries, so they also run on routers and in containers. The aarch64 and Apple-silicon builds use mimalloc, the others jemalloc, the same as upstream's CI.

**Kept small on purpose.** CI fails a `lolinet` binary over 32 MiB or a package over 14 MiB, and fails a Linux build that isn't statically linked. Anything with a GUI or system-framework dependencies, such as [lolidesk](lolidesk.md), ships as a separate package.

**The service.** `lolinet up` copies itself to `/usr/local/bin/lolinet` (write to a temp file, then rename, so a running old version isn't disturbed), puts the config at `/etc/lolinet/loli.toml` with mode 600, and registers the LaunchDaemon `co.lo-li.lolinet` (macOS) or `lolinet.service` (Linux). Both run `lolinet run`, which starts the kernel with `--rpc-portal 127.0.0.1:15888`: EasyTier v2.7.0 ignores `rpc_portal` in the config file and would otherwise listen on `0.0.0.0`. The kernel logs only to `/var/log/lolinet/easytier.log`, 10 MB × 5. Its terminal log is off, because hole-punching warnings are frequent and launchd doesn't rotate stderr; argument errors and panics still reach launchd or journald.

**Taking over older installs.** Earlier versions of the join script installed `easytier-core` and `easytier-cli` as a service named `easytier`. `lolinet up`:

- removes that service, which would compete for the same TUN device and credential;
- copies `/etc/easytier/loli.toml` to `/etc/lolinet/loli.toml` if the new one doesn't exist yet, and leaves the old file in place;
- replaces `easytier-core` and `easytier-cli` in the old directory (`/usr/local/libexec/easytier/` on macOS, `/opt/easytier/` on Linux) with symlinks to `lolinet`, so the probe and older scripts keep working.

**Credential expiry.** `pair.py` appends `# credential_id` and `# credential_expiry_unix` to the config as TOML comments. EasyTier ignores them; `lolinet status` reads them to show the days left. The config is mode 600, so this line needs `sudo`.

## Pairing

```
admin                         console (issuer)        join site (Worker + KV)         device
  │ pair.py new ─────────────▶ new credential
  │ render config, encrypt with a fresh code
  │ PUT pair:<sha256(code)[:32]> = ciphertext, TTL 15 min ─────▶
  │ shows: curl …/i | sudo sh -s CODE
  │                                                                   ◀── GET /i
  │                                                                   ◀── GET /v1/pair/<sha256(code)[:32]>
  │                                                  ciphertext, then delete ──▶ openssl decrypt with CODE
  │                                                                       download lolinet, check sha256
  │                                                                       lolinet up -c loli.toml
```

- **Code**: 12 characters from an alphabet without look-alikes (no `0`, `O`, `1`, `I`, `L`), shown in groups of four.
- **Encryption**: PBKDF2-SHA256 with 200,000 rounds derives the key and IV, then AES-256-CBC, in OpenSSL's `Salted__` format. The device decrypts with `openssl enc -d -aes-256-cbc -pbkdf2 -iter 200000 -md sha256`, which every macOS and Linux has.
- **The Worker** stores the ciphertext under a hash of the code, deletes it on the first `GET`, and allows each IP 20 attempts per 10 minutes. `HEAD` requests are refused so they can't use up a code. The code itself never reaches the Worker.
- **The probe** rides along: unless the code was made with `--no-probe`, the config ends with a `# probe = {…}` line holding the device's public id and report key. `install.sh` and `lolinet up` take that line out before the config goes to EasyTier.
- **Before redeeming**, `install.sh` checks that the machine is supported (macOS, or Linux with systemd), so a code isn't spent on a machine that can't use it.

## Auto-upgrade

`lolinet up` installs a job that runs `lolinet upgrade --auto` every 6 hours: the LaunchDaemon `co.lo-li.lolinet-upgrade`, or `lolinet-upgrade.timer` with up to 30 minutes of random delay.

**The manifest** (`lolinet.json`, written by [`lolinet/release.py`](../lolinet/release.py)) names the version devices should run. `lolinet` doesn't compare old and new: if the local version differs, it switches. Rolling back means publishing the old version again.

**Where it looks.** The settings in `/etc/lolinet/lolinet.json` decide: an explicit source (`github:<owner>/<repo>` or a URL), else the join site's `<base_url>/dl/lolinet.json`, else GitHub `prnake/lolinet`. Packages are resolved next to the manifest. GitHub's `releases/latest/download/lolinet.json` first redirects to `releases/download/<tag>/lolinet.json`, which pins the version, and then to a one-time storage URL. `lolinet` follows only redirects that keep the file name, so it builds package URLs from the pinned directory and never from the storage URL.

**Three checks before switching:**

1. Download with the system's `curl`.
2. The sha256 must match the manifest.
3. The new binary must run on this machine and print the manifest's version from `--version`.

Then it replaces `/usr/local/bin/lolinet` atomically and restarts the service. If the probe is installed and the release's `reporter.py` differs, that is replaced too and the probe restarted. The job logs only switches and errors.

## Public probe

The join site's home page shows the network's health to anyone, without naming anything.

**Reports.** [`probe/reporter.py`](../probe/reporter.py) needs only the Python standard library (macOS's `/usr/bin/python3` 3.9 is enough) and runs without root.

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

This repository holds code only. A deployment is the code plus a node list (`nodes.yaml`, or `LOLINET_NODES`) and an admin directory (`$LOLINET_HOME`) that hold everything specific to one network. All packages are built and released by this repository's workflows; the join site's download area is a copy of a release, uploaded unchanged with `join/deploy.py release`.
