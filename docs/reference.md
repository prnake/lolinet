# Reference

Every command, setting, file and endpoint, in one place. For a walkthrough see [Self-hosting](self-hosting.md); for how things work see [Architecture](architecture.md).

- [`lolinet` CLI](#lolinet-cli)
- [Files on a device](#files-on-a-device)
- [`nodes.yaml`](#nodesyaml)
- [Environment variables](#environment-variables)
- [Admin state](#admin-state)
- [Admin scripts](#admin-scripts)
- [Join site](#join-site)
- [Release artifacts](#release-artifacts)
- [Ports](#ports)

## `lolinet` CLI

`lolinet` with no command is `lolinet status`. Commands marked *root* need `sudo`.

| Command | |
| :-- | :-- |
| `lolinet status` | The service, this machine, days left on the credential, the upgrade setting, and every relay and device: direct or relayed, latency, loss |
| `lolinet peers` | `easytier-cli peer` |
| `lolinet routes` | `easytier-cli route` |
| `lolinet logs [-f] [-n 50]` | The kernel log, `/var/log/lolinet/easytier.log` |
| `lolinet up [options]` *root* | Install the binary and the service and start it. Takes over an `easytier` service that an earlier join script installed. Prints the status at the end |
| `lolinet down` *root* | Stop and remove the service and the probe. The config and binary stay |
| `lolinet restart` *root* | Restart the kernel |
| `lolinet upgrade [options]` *root* | Switch to the version the release manifest names (see below). Also updates lolidesk, if installed |
| `lolinet probe [show\|send] [-c FILE]` *root* | Print what the public probe would report this round; `send` reports it once for real. Reads `/etc/lolinet/probe.json` |
| `lolinet probe off` *root* | Take the device off the public page, remove the probe service and delete its config. Turning it back on needs a new pairing |
| `lolinet probe run` | The probe service's entry point |
| `lolinet invite [--kind desktop\|userspace\|server] [--name N] [--manager URL[@IP]] [--json]` | Ask the network's manager for a pairing code for another machine; the address is assigned centrally. No root and no console account: the manager knows this node by its overlay address. `--manager` is needed only on devices paired before this existed; run as root, it is remembered |
| `lolinet exit` | Whether this node is an exit node, its address and service state |
| `lolinet exit on [--port 10180] [--allow CIDR]… [--lan]` *root* | Make this node an exit node: a SOCKS5 server (`CONNECT` and `UDP ASSOCIATE`) for the overlay, installed as a service. `--allow` limits who may use it (default the overlay's CIDR); `--lan` also lets users reach private addresses behind this node |
| `lolinet exit off` *root* | Stop and remove it |
| `lolinet exit run [--listen ADDR] [--allow CIDR]… [--lan]` | The exit service's entry point. With `--listen` it ignores the config and allows loopback by default (for tests) |
| `lolinet run [-c FILE]` | The service's entry point: run the EasyTier kernel in the foreground with `/etc/lolinet/loli.toml` |
| `lolinet core …` | Stock `easytier-core`, arguments passed through |
| `lolinet cli …` | Stock `easytier-cli`, arguments passed through, aimed at the local service's admin port |
| `lolinet --version` | The version, from `git describe` at build time |

The same binary called `easytier-core` or `easytier-cli` (by symlink or by name) is the stock tool.

**`lolinet up` options.** Each one, when left out, keeps its previous value from `/etc/lolinet/lolinet.json`.

| Option | |
| :-- | :-- |
| `-c, --config FILE` | Use this config (the `loli.toml` from pairing). It is copied to `/etc/lolinet/loli.toml` with mode 600. A `# probe = {…}` line is moved to `/etc/lolinet/probe.json` and the probe installed; without one, an installed probe is left alone. A `# lolinet = {…}` line (the manager's address) goes to `/etc/lolinet/lolinet.json` |
| `--base-url URL` | The join site. Also read from `LOLINET_BASE_URL`; `install.sh` passes it this way |
| `--upgrade-source SRC` | `github:<owner>/<repo>`, a manifest URL, or `base` to use the join site |
| `--auto-upgrade on\|off` | The 6-hourly upgrade check. On by default |
| `--userspace` | Join without a TUN interface: programs on this machine reach the overlay through SOCKS5 on `127.0.0.1:18080`, and connections to this node's overlay address are delivered to `127.0.0.1`. Also read from `LOLINET_USERSPACE=1`, so `curl … \| sudo LOLINET_USERSPACE=1 sh -s CODE` pairs in this mode. Writes `socks5_proxy` and `[flags] no_tun = true` into `loli.toml` |
| `--tun` | Back to TUN mode, the default: removes those two settings |

**`lolinet upgrade` options.**

| Option | |
| :-- | :-- |
| `--check` | Show the published version and stop. No root needed |
| `--dry-run` | Download, check the sha256, test-run the new binary, but don't switch. No root needed |
| `--source SRC` | Use another source this time: `github:<owner>/<repo>` or a manifest URL |

**Where the manifest comes from**, first match wins: the `--upgrade-source` you set; the join site's `<base_url>/dl/lolinet.json`; GitHub `prnake/lolinet`. `github:<owner>/<repo>` means `https://github.com/<owner>/<repo>/releases/latest/download/lolinet.json`.

## Files on a device

| Path | |
| :-- | :-- |
| `/usr/local/bin/lolinet` | The binary |
| `/etc/lolinet/loli.toml` | EasyTier config, mode 600. The last lines hold `# credential_id` and `# credential_expiry_unix`, which `lolinet status` reads |
| `/etc/lolinet/lolinet.json` | Settings: `base_url`, `source`, `auto_upgrade`, and the manager `lolinet invite` asks (`manager`, `manager_ip`, from the pairing config's `# lolinet = {…}` line). Mode 644, no secrets |
| `/var/log/lolinet/easytier.log` | Kernel log, 10 MB × 5 files |
| `/var/log/lolinet/upgrade.log` | Upgrade log on macOS (journald on Linux). Written only on a switch or an error |
| `/Library/LaunchDaemons/co.lo-li.lolinet.plist` | macOS service, RunAtLoad and KeepAlive |
| `/Library/LaunchDaemons/co.lo-li.lolinet-upgrade.plist` | macOS upgrade job |
| `/etc/systemd/system/lolinet.service` | Linux service, `Restart=always` |
| `/etc/systemd/system/lolinet-upgrade.{service,timer}` | Linux upgrade job, every 6 hours with up to 30 minutes of random delay |

The exit node, when turned on. It runs `/usr/local/bin/lolinet exit run`, starts as root, binds its port and drops to `nobody`. It listens on the overlay address in TUN mode and on `127.0.0.1` in userspace mode:

| Path | macOS | Linux |
| :-- | :-- | :-- |
| Settings (`port`, `allow`, `lan`) | `/etc/lolinet/exit.json`, mode 644 | `/etc/lolinet/exit.json`, mode 644 |
| Service | `/Library/LaunchDaemons/co.lo-li.lolinet-exit.plist` | `/etc/systemd/system/lolinet-exit.service` |
| Log | `/var/log/lolinet/exit.log`, errors only | journald |

The public probe, when installed. It runs `/usr/local/bin/lolinet probe run`, starts as root and drops to `nobody` after reading its config:

| Path | macOS | Linux |
| :-- | :-- | :-- |
| Config (id and report key) | `/etc/lolinet/probe.json`, mode 600 | `/etc/lolinet/probe.json`, mode 600 |
| Service | `/Library/LaunchDaemons/co.lo-li.lolinet-probe.plist` | `/etc/systemd/system/lolinet-probe.service` |
| Log | `/var/log/lolinet/probe.log` | journald |

Earlier installs ran the Python reporter instead (`reporter.py` under `/usr/local/libexec/mesh-probe/` or `/opt/mesh-probe/`, config `/etc/mesh-probe/reporter.json`, service `co.lo-li.mesh-probe` or `mesh-probe.service`). `lolinet up` and `lolinet upgrade` move a device-mode config to `/etc/lolinet/probe.json`, keeping the public id, and remove the old service. Relay-mode configs are not touched.

lolidesk, once a lolidesk release is published:

| Path | |
| :-- | :-- |
| `/usr/local/lib/lolidesk/` | lolidesk (`lolidesk` and `libghostty-vt`), linked from `/usr/local/bin/lolidesk`. `install.sh` installs it but doesn't start the host. `lolinet upgrade` replaces it when `lolidesk.json` names another version |
| `/usr/local/bin/lolidesk-host` | Older installs only: now a link to `lolidesk`, which acts as `lolidesk host` under that name |
| `/usr/local/bin/lolidesk` | The lolidesk client, updated the same way |

Paths from before `lolinet`, which `lolinet up` takes over: the config `/etc/easytier/loli.toml` (copied if `/etc/lolinet/loli.toml` doesn't exist yet), and the binary directory `/usr/local/libexec/easytier/` on macOS or `/opt/easytier/` on Linux, where `easytier-core` and `easytier-cli` become symlinks to `lolinet`.

## `nodes.yaml`

The node list is the single source for the scripts, the Worker and the web build. They look for `LOLINET_NODES`, then `nodes.yaml` at the repository root, then `nodes.example.yaml`.

### `network`

| Key | |
| :-- | :-- |
| `name` | EasyTier `network_name`, also the instance name in device configs |
| `title` | Display name on the local console, the public probe page, page titles and the join site's pages. Default `lolinet` |
| `cidr` | The overlay range, for example `10.188.0.0/16`. Device configs always use a `/16` prefix |
| `sni` | TLS server name devices send on WSS (`?sni=` on each peer URL) |
| `easytier` | Informational: the EasyTier version the network runs |
| `secure_mode`, `private_mode` | Informational: the relays' modes. Device configs always set both |
| `edge_zone` | `<relay>.<edge_zone>` points at each relay's public address. The Worker also serves `<name>.<edge_zone>` redirect pages |
| `local_suffix` | `<name>.<edge_zone>` redirects to `<name><local_suffix>`, a name that resolves to an overlay address |
| `join` | The join site's host. `LOLINET_BASE_URL` overrides it |
| `console` | The easytier-web console's URL, reachable on the overlay only |
| `manager` | The manager's URL on the overlay (`web/agent/mesh_manager.py` on the issuer relay). When set, `pair.py` ends each config with `# lolinet = {…}` so the device's `lolinet invite` knows where to ask |
| `issuer` | The relay that runs the console and signs credentials. Scripts connect to its overlay IP and use the console's name for SNI and certificate checks |
| `device_peers` | Relays a new device connects to. Default: the first three relays |

### `cloudflare`

| Key | |
| :-- | :-- |
| `zone` | The zone the custom domains live in. Default: the last two labels of the join host |
| `worker` | Worker script name. Default `lolinet-join` |
| `kv` | KV namespace title for pairing ciphertext. Default `lolinet-pairing` |
| `bucket` | R2 bucket for `install.sh`, packages and the probe page. Default `lolinet-dist` |
| `redirects` | Extra `<name>.<edge_zone>` hosts bound to the Worker as redirect pages, for example `[console]` |

### `relay_defaults` and `relays`

| Key | |
| :-- | :-- |
| `name` | The relay's name. The Worker uses it only to check report signatures; it is never published |
| `region` | Cloud region code, for example `cn-beijing`. It decides the city and globe position on the public page ([`web/src/data/regions.ts`](../web/src/data/regions.ts)) and is the relay's public id |
| `zone`, `net` | Informational: availability zone, and the VPC as `10.<net>.0.0/16` |
| `overlay_ip` | The relay's overlay address |
| `wss_port` | Informational: the WSS port devices use. Device configs use 443 |
| `provider`, `instance_type` | Shown on the relay's probe card as "provider instance_type". Each falls back to `relay_defaults`; a missing part is left out |

### `devices`

Optional. `device_config.py` renders configs for devices listed here; `pair.py` doesn't need them.

| Key | |
| :-- | :-- |
| `name`, `overlay_ip` | Hostname and overlay address |
| `os`, `mode` | Informational: platform, and `tun` or not |
| `peers` | Relays to connect to |
| `transport` | `wss` (default) or `tcp` for stock EasyTier without the SNI patch |

## Environment variables

| Variable | Used by | |
| :-- | :-- | :-- |
| `LOLINET_BASE_URL` | scripts, web build, `install.sh`, `lolinet up` | The join site, for example `https://join.edge.example.com`. Default `https://<network.join>`. The Worker writes it into `install.sh` when serving `/i` |
| `LOLINET_USERSPACE` | `lolinet up`, so also `install.sh` | `1` joins without TUN, as `lolinet up --userspace` |
| `LOLINET_GITHUB` | `install.sh` | The GitHub repository to download `lolinet` from. Default `prnake/lolinet` |
| `LOLINET_NODES` | scripts, web build | Path to the node list |
| `LOLINET_HOME` | scripts, agent | Admin data directory with `.env` and `state/`. Default `~/.config/lolinet`. `FLEET_REPO` is still read as a fallback |
| `CLOUDFLARE_API_TOKEN`, `CLOUDFLARE_ACCOUNT_ID` | `deploy.py`, `pair.py` | Cloudflare credentials. In `$LOLINET_HOME/.env` they are `CLOUDFLARE_API` and `CLOUDFLARE_ID` |
| `PROBE_MASTER` | `deploy.py` | The probe master key, 64 lowercase hex digits. Required in CI; locally it comes from `state/mesh-probe.json` and is generated if missing |
| `ET_WSS_SNI` | patched `easytier-core` | Default SNI for WSS URLs without `?sni=`, such as peer URLs learned from other nodes |
| `LOLINET_VERSION` | `lolinet` build | The version compiled in; `build.sh` and CI set it from `git describe` |
| `LOLIDESK_VERSION` | lolidesk build | The version `lolidesk --version` prints; `make desk` and CI set it from `git describe`, otherwise it is the Cargo version |
| `MESH_AGENT` | `npm run dev` | Where the dev server proxies `/api`. Default `http://127.0.0.1:10188` |
| `MESH_JOIN` | `npm run dev:probe` | Where the probe dev server proxies `/v1`. Default: the join site |

## Admin state

Files in `$LOLINET_HOME/state/`, all mode 600, never in git:

| File | Written by | |
| :-- | :-- | :-- |
| `provision-<region>-<name>.json` | you | One per relay: `{"name", "role": "mesh-relay", "eip", "et_port"}` |
| `mesh-console.json` | you | Console login and network: `admin_user`, `admin_pw`, `network_id` |
| `mesh-probe.json` | `deploy.py worker` | The probe master key, `{"master": …}` |
| `mesh-devices.json` | `pair.py` | Device registry: name, overlay IP, credential id, expiry, pairing time. No keys |
| `mesh-cred-<name>.json` | you | Optional: a credential for `device_config.py` |

## Admin scripts

Run them with `uv run`; each declares its own dependencies.

| Command | |
| :-- | :-- |
| `scripts/pair.py new --name N --ip IP [options]` | Sign a credential, render the config, encrypt it with a new pairing code, store it in KV. Options: `--peers`, `--ttl-days` (365), `--code-minutes` (15, max 1440), `--socks HOST:PORT`, `--transport wss\|tcp`, `--no-probe`, `--location CITY`, `--out FILE` (write the config instead of making a code) |
| `scripts/revoke.py NAME SOCKS\|- [CREDENTIAL_ID]` | Revoke the device's latest credential (or the one given) and remove it from the registry |
| `scripts/device_config.py --name N --out FILE [--cred ID] [--by-name]` | Render a config for a device in `nodes.yaml` from a saved credential |
| `join/deploy.py worker` | Upload the Worker with its bindings, attach the custom domains, turn off `workers.dev` |
| `join/deploy.py site` | Upload `web/dist-probe/` as the join site's home page |
| `join/deploy.py release DIR` | Upload `lolinet-*` and `easytier-*` packages, then `dl/lolinet.json` |
| `join/deploy.py install` | Upload `install.sh`, once all four `lolinet` packages and `dl/lolinet.json` are on the join site |
| `join/deploy.py lolidesk DIR` | Upload the four `lolidesk-*` packages from a lolidesk release, then `dl/lolidesk.json` |
| `join/deploy.py dist FILE…` | Upload any files to `dl/` with a `.sha256` next to each |
| `probe/device.py relay --name N --out FILE` | A relay's probe config: report key, device-id key and report URL |
| `probe/device.py config --name N --out FILE [options]` | A device's probe config for `reporter.py`, for devices without `lolinet` (Docker) |
| `probe/device.py id --name N` | The public id a hostname gets |
| `probe/device.py bye --name N` | Remove a device from the public page |
| `probe/device.py install-mac` / `uninstall-mac` | Run the probe as a user LaunchAgent on this Mac, without root |
| `lolinet/build.sh` | Build `lolinet` for this machine (`TARGET`, `FEATURES` to cross-compile) |
| `lolinet/release.py DIR` | Write `lolinet.json` next to four `lolinet-*` packages |
| `easytier/check_sni.py PATH` | Start a fake relay and check that `easytier-core` sends the right SNI |
| `sudo lolinet/check_probe.py PATH` | Install the built-in probe from this `lolinet` for real and check it end to end. Changes system services; meant for CI runners |

## Join site

The Worker answers on the join host and on every `<name>.<edge_zone>` bound to it.

| Request | |
| :-- | :-- |
| `GET /` | The public probe page for browsers; for anything else, a short text with the install command |
| `GET /i` | `install.sh`, with the join site's address filled in. See [`install.sh`](#installsh) |
| `GET /dl/<file>` | Packages, `.sha256` files and `lolinet.json` from R2 |
| `GET /v1/pair/<id>` | The pairing ciphertext, once. `<id>` is the first 32 hex digits of the code's sha256. 20 attempts per IP per 10 minutes |
| `GET /v1/probe` | The public snapshot: relays by region, devices by anonymous id, links, availability. Cached 10 seconds, CORS open |
| `GET /v1/probe/link?a=&b=` | 24 hours of latency between two relays, both directions |
| `POST /v1/probe/report` | Signed reports from relays and devices (`X-Probe-Node`, `X-Probe-Ts`, `X-Probe-Sig`). At most 16 KiB, clock skew at most 5 minutes |
| `GET /favicon.svg`, `/assets/*` | The probe page's files |
| any path on `<name>.<edge_zone>` | A page that checks whether `<name><local_suffix>` answers on the local network and goes there if it does |

**Worker bindings** set by `deploy.py worker`: `PAIRING` (KV), `DIST` (R2), `PROBE` (the `ProbeHub` Durable Object), `PROBE_MASTER` (secret), `RELAYS` (name, region and spec per relay), `BASE_URL`, `EDGE_ZONE` and `TITLE`.

**Probe history.** Relays count as online for 90 seconds after a report, devices for 180. A device that hasn't reported for 7 days leaves the page. Heartbeats are kept 31 days in 5-minute buckets, link latency 2 days. A "day" on the page starts at 00:00 UTC+8.

## `install.sh`

| Command | |
| :-- | :-- |
| `curl <join site>/i \| sudo sh -s CODE` | New device: download and check `lolinet`, then redeem the code, `lolinet up -c` the config, which installs the probe |
| `curl <join site>/i \| sudo sh` | Device already in: use `/etc/lolinet/loli.toml` (or the older `/etc/easytier/loli.toml`) and run `lolinet up`. Does nothing when the version matches and the service, upgrade job and probe (if configured) are in place. Takes over an older Python probe. Refuses relay configs (they hold `network_secret`). `repair` and `upgrade` are accepted as the argument too |
| `curl <join site>/i \| sh -s check` | Download and check only, then print the latest version, the local one and whether to install. No root needed |

**Where it downloads.** GitHub first: the redirect of `https://github.com/$LOLINET_GITHUB/releases/latest` pins the tag, and every file comes from that release. If GitHub is unreachable, slower than 50 KB/s for 20 seconds, or has no usable manifest, it uses `<join site>/dl/`. The package's sha256 must match that release's `lolinet.json`, and the new binary must print the manifest's version. `lolinet up` gets `--upgrade-source github:<repo>` or `base` to match where the download came from.

**lolidesk.** All three commands also handle lolidesk. They read `lolidesk.json` from the newest `lolidesk-*` release on GitHub (through the API, since these releases don't take `latest`), or from `<join site>/dl/` when `lolinet` came from there or GitHub has none. Pairing and `sudo sh` install lolidesk to `/usr/local/lib/lolidesk/` (linked from `/usr/local/bin/lolidesk`), or replace it, when its `--version` differs or the old separate binaries are still in `/usr/local/bin`; it must pass the sha256 check and a test run first, and is skipped otherwise. `check` says whether they would be installed or updated. A lolidesk problem never fails the script.

## Release artifacts

A `lolinet-<version>` GitHub Release, and the join site's `dl/`, hold the same files:

| File | |
| :-- | :-- |
| `lolinet-<os>-<arch>.tar.gz` | `lolinet` and `BUILD-INFO` at the archive root. `<os>-<arch>` is `linux-x86_64`, `linux-aarch64`, `macos-aarch64` or `macos-x86_64` |
| `easytier-<os>-<arch>.tar.gz` | The patched `easytier-core` and `easytier-cli`, and `BUILD-INFO` |
| `*.sha256`, `SHA256SUMS` | Checksums |
| `lolinet.json` | The manifest |

`BUILD-INFO`'s first line is `lolinet <version>`, followed by the EasyTier tag and commit, the patches, the target and the CI run. The manifest:

```json
{
 "version": "<lolinet --version>",
 "lolinet": {
  "macos-aarch64": {"file": "lolinet-macos-aarch64.tar.gz", "sha256": "…"},
  "…": {}
 }
}
```

All four packages must carry the same version. Older manifests also had a `"probe"` entry for the Python probe bundle; the probe is now part of `lolinet`, and older `lolinet` versions ignore a manifest without it. Package URLs are resolved next to the manifest's URL, so a manifest and its packages always sit in one directory.

The `dist` workflow uploads every release to the join site's `dl/` unchanged, packages first and the manifest last, so a device never sees a manifest whose packages aren't there yet.

lolidesk ships as `lolidesk-<version>` releases, published by the `desk` workflow on every push to `main` that changes `desk/` and never marked latest. Each holds `lolidesk-<os>-<arch>.tar.gz` for the same four platforms (`lolidesk`, `libghostty-vt.dylib` or `.so`, `BUILD-INFO` whose first line is `lolidesk <version>`, and `Lolidesk.app` on macOS), `*.sha256`, `SHA256SUMS` and the manifest `lolidesk.json`:

```json
{
 "version": "<lolidesk --version>",
 "lolidesk": {
  "macos-aarch64": {"file": "lolidesk-macos-aarch64.tar.gz", "sha256": "…"},
  "…": {}
 }
}
```

On the join site these sit in `dl/` next to `lolinet.json`. See [lolidesk](lolidesk.md#distribution).

## Ports

| Port | |
| :-- | :-- |
| `443` | WSS from devices to relays; HTTPS for the console on the issuer's overlay address |
| `127.0.0.1:15888` | Admin port of the `lolinet` service |
| `127.0.0.1:15897` | Admin port by convention for a separate no-tun instance (one without a TUN interface, reachable through SOCKS5), such as the old menu-bar app's |
| `127.0.0.1:18080` | SOCKS5 into the overlay on a node in userspace mode (`lolinet up --userspace`), and by convention for a separate no-tun instance; pass it to the scripts with `--socks` |
| `10180` | Exit node SOCKS5 (`lolinet exit on`), on the node's overlay address |
| `127.0.0.1:10188` | The local console's agent |
| `127.0.0.1:11211` | easytier-web's API on the issuer relay |
| `10190` | `lolidesk host` (terminal, desktop and computer use on one QUIC port), on the host's overlay address |
| `5188`, `5189` | Dev servers for the console and the probe page |

### Address plan

The example network uses `10.188.0.0/16`:

| Range | |
| :-- | :-- |
| `10.188.0.0/24` | Relays and infrastructure |
| `10.188.1.0/24` | Servers |
| `10.188.16.0/20` | Computers with TUN |
| `10.188.32.0/20` | Clients without TUN |
| `10.188.48.0/20` | Phones, through a relay's WireGuard portal (planned, see [Design](design.md#mihomo-on-phones-and-desktops)) |

`lolinet status` and the probe reporter treat `10.188.0.0/24` as the relay range, and the console's Pair page picks the next free address from the computer or client range.
