# Self-hosting

This page sets up a lolinet network of your own: relays, a console that signs credentials, a join site on Cloudflare, and the first devices. Names, files and flags are listed in the [reference](reference.md); how the pieces work is in [architecture](architecture.md).

The examples use `edge.example.com` as the edge zone and `join.edge.example.com` as the join site. The reference network at [join.edge.lo-li.co](https://join.edge.lo-li.co) is built exactly this way, with six relays.

## What you need

| | |
| :-- | :-- |
| **Relays** | Two or more Linux hosts with public addresses. Each one listens for EasyTier over TCP, UDP and WSS. Devices connect on 443 (WSS); relays reach each other on their TCP and UDP ports, as do devices paired with `--transport tcp` |
| **A domain on Cloudflare** | The join site runs on Workers with KV, R2 and a Durable Object. You need an API token with Workers Scripts, Workers Routes, Workers KV and R2 edit permission |
| **An admin machine** | [uv](https://docs.astral.sh/uv/) for the scripts (Python 3.11+), Node 24 and npm for the web build, and optionally the [`gh`](https://cli.github.com) CLI to download releases |
| **Packages** | The `lolinet` packages from this repository's [Releases](https://github.com/prnake/lolinet/releases), or your own build (see [Building packages](#building-packages)) |

Two directories hold everything that is yours and not in git:

- **`nodes.yaml`**: the node list. Copy [`nodes.example.yaml`](../nodes.example.yaml) to `nodes.yaml` at the repository root (it is git-ignored), or keep it anywhere and point `LOLINET_NODES` at it.
- **`$LOLINET_HOME`** (default `~/.config/lolinet`): `.env` with your Cloudflare credentials and `state/` with relay addresses, the console login and the probe master key. Every file in `state/` is mode 600.

## 1 · Write the node list

```sh
cp nodes.example.yaml nodes.yaml
```

Change at least these:

| Key | Set it to |
| :-- | :-- |
| `network.name`, `network.title` | The EasyTier network name, and the name people see on the console and probe page |
| `network.cidr` | Your overlay range. The example uses `10.188.0.0/16`, with relays in `10.188.0.0/24` |
| `network.sni` | The TLS server name devices send when they connect to a relay over WSS |
| `network.edge_zone`, `network.local_suffix`, `network.join`, `network.console` | Your domain: `edge.example.com`, `-local.edge.example.com`, `join.edge.example.com`, `https://console-local.edge.example.com` |
| `network.issuer`, `network.device_peers` | The relay that runs the console, and the relays a new device connects to by default |
| `cloudflare.*` | Your zone, and the names of the Worker, KV namespace and R2 bucket |
| `relay_defaults`, `relays` | One entry per relay: name, region (decides the city on the public page), overlay IP. Optionally `provider` and `instance_type` for the probe card |

Every key is described in the [reference](reference.md#nodesyaml). Set `LOLINET_BASE_URL` (for example `https://join.edge.example.com`) if the join site's address differs from `https://<network.join>`.

## 2 · Set up the relays

The scripts that create relays are tied to one cloud provider, so they aren't in this repository. Whatever you use, each relay needs:

1. **EasyTier v2.7.0.** The `easytier-<platform>.tar.gz` packages in a release are the patched build of the same version.
2. **Secure mode and private mode**, listening on TCP, UDP and WSS (443), with every relay connected to every other relay.
3. **The admin port on loopback.** EasyTier v2.7.0 ignores `rpc_portal` in the config file, so pass `--rpc-portal 127.0.0.1:15888` on the command line. Otherwise it listens on `0.0.0.0`.
4. **A DNS name**: `<relay>.edge.example.com` pointing at its public address.

The issuer relay (`network.issuer`) also runs the console, `easytier-web-embed`:

- Use Gateway mode for the network, and make the console listen only on the overlay.
- Put HTTPS in front of it on the issuer's overlay address, port 443, with a certificate valid for the console's name (a wildcard `*.edge.example.com` from Let's Encrypt over DNS-01 works). `pair.py`, `revoke.py` and the local console's agent all verify it.
- Point `console-local.edge.example.com` at the issuer's overlay IP, DNS only (not proxied).

Then record each relay in `$LOLINET_HOME/state/`:

```sh
# one file per relay: provision-<region>-<name>.json
echo '{"name": "chen", "role": "mesh-relay", "eip": "203.0.113.11", "et_port": 11010}' \
  > ~/.config/lolinet/state/provision-cn-beijing-chen.json

# the console login and network: mesh-console.json
echo '{"admin_user": "…", "admin_pw": "…", "network_id": "…"}' > ~/.config/lolinet/state/mesh-console.json
chmod 600 ~/.config/lolinet/state/*.json
```

`pair.py` writes device configs from these: each peer URL is `wss://<eip>:443/?sni=<network.sni>`, or `tcp://<eip>:<et_port>` for `--transport tcp`.

## 3 · Deploy the join site

In Cloudflare, create the KV namespace and the R2 bucket named in `cloudflare.kv` and `cloudflare.bucket`. Give the scripts your credentials as `CLOUDFLARE_API_TOKEN` and `CLOUDFLARE_ACCOUNT_ID`, or in `$LOLINET_HOME/.env`:

```sh
CLOUDFLARE_API=…     # API token
CLOUDFLARE_ID=…      # account id
```

Then:

```sh
export LOLINET_BASE_URL=https://join.edge.example.com
uv run join/deploy.py worker                          # the Worker, KV / R2 / Durable Object bindings, custom domains
(cd web && npm ci && npm run build:probe) && uv run join/deploy.py site   # the public probe page
gh release download <lolinet-… tag> --repo prnake/lolinet --dir out   # a lolinet release, or your own build
uv run join/deploy.py release out                     # packages, probe bundle, then the manifest dl/lolinet.json
uv run join/deploy.py install                         # install.sh, once the packages are there
```

- `deploy.py worker` creates the probe master key in `state/mesh-probe.json` the first time. In CI, pass it as `PROBE_MASTER` instead; CI never generates one.
- `deploy.py install` refuses to upload `install.sh` until all four `lolinet` packages are on the join site, so new devices never get a script that can't download its package.
- `deploy.py release` uploads the manifest last, so a device that sees a new manifest always finds its packages.

Open `https://join.edge.example.com` in a browser for the probe page, or `curl` it for the install command.

## 4 · Turn on the relay probes

Each relay reports its load and links to the join site. Make its config on the admin machine:

```sh
uv run probe/device.py relay --name chen --out /tmp/chen.json   # mode 600, holds the relay's report key
```

On the relay, install the reporter and its systemd unit, and put the config where the unit reads it:

```sh
install -d -m 755 /opt/mesh-probe && install -m 644 reporter.py /opt/mesh-probe/reporter.py
install -d -m 700 /etc/mesh-probe && install -m 600 chen.json /etc/mesh-probe/reporter.json
install -m 644 mesh-probe.service /etc/systemd/system/mesh-probe.service
systemctl daemon-reload && systemctl enable --now mesh-probe
```

The reporter reads peers from `/opt/easytier/easytier-cli` on `127.0.0.1:15888`, and runs as a dynamic user with no root. `python3 reporter.py --once --config /etc/mesh-probe/reporter.json` prints what it would send, without sending it.

## 5 · Add devices

`pair.py` talks to the console over the overlay. For the first device, when nothing of yours is on the network yet, open a SOCKS proxy through the issuer relay:

```sh
ssh -N -D 127.0.0.1:18080 root@<issuer public address> &
uv run scripts/pair.py new --name laptop --ip 10.188.16.10 --socks 127.0.0.1:18080
```

Run the printed command on the device:

```sh
curl -fsSL https://join.edge.example.com/i | sudo sh -s XXXX-XXXX-XXXX
```

Once your admin machine is on the network, drop `--socks`. The [local console](console.md) can do the same from its **Pair** page.

| Option | |
| :-- | :-- |
| `--ip` | The device's overlay address. The example plan: `10.188.1.0/24` for servers, `10.188.16.0/20` for computers with TUN, `10.188.32.0/20` for clients without TUN |
| `--peers chen,saria` | Relays to connect to, instead of `network.device_peers` |
| `--ttl-days 365` | Credential lifetime |
| `--code-minutes 15` | How long the code works (1–1440) |
| `--no-probe` | Don't install the public probe |
| `--location 上海` | City to show on the probe page, instead of "near <nearest relay>" |

`pair.py` adds each device to `state/mesh-devices.json` (no keys). To revoke:

```sh
uv run scripts/revoke.py laptop -                  # - when this machine is on the network, else a SOCKS address
uv run scripts/revoke.py laptop - <credential id>  # one of several credentials with the same name
```

### Devices without systemd

`install.sh` needs launchd or systemd. For Docker, or a NAS without sudo, use the official EasyTier image over TCP (it has no SNI patch) and hand the config over yourself:

```sh
uv run scripts/pair.py new --name nas --ip 10.188.1.10 --transport tcp --peers chen,hoshiguma,saria \
  --out /tmp/nas.toml --socks 127.0.0.1:18080
```

The config is written with mode 600 and no pairing code is made. Copy it to the device and delete the local file. To show the device on the probe page, make its probe config with `uv run probe/device.py config --name nas …` and run `reporter.py` there.

## 6 · Watch it

On any device, `lolinet` shows the service, this machine, the credential's days left, and every relay and device with direct or relayed, latency and loss.

On your admin machine, the local console shows the whole network in a browser:

```sh
(cd web && npm ci && npm run build)
python3 web/agent/mesh_agent.py --allow-pair
open http://127.0.0.1:10188/
```

See [Local console](console.md) for its flags.

## 7 · Upgrade devices

Devices installed by `install.sh` check every 6 hours where they were installed from (GitHub's latest release, or the join site's `dl/lolinet.json`) and switch to the version it names. The same run updates the probe and any installed lolidesk binaries.

**Let CI keep the join site in step.** The [`dist`](../.github/workflows/dist.yml) workflow uploads every lolinet and lolidesk release to the join site's `dl/` as soon as it is published, and uploads `install.sh` when a change to it reaches `main`. Add these repository secrets (Settings → Secrets and variables → Actions):

| Secret | |
| :-- | :-- |
| `CLOUDFLARE_API_TOKEN` | A token with write access to the join site's R2 bucket. Nothing else is needed |
| `CLOUDFLARE_ACCOUNT_ID` | The Cloudflare account |
| `LOLINET_BASE_URL` | The join site, for example `https://join.edge.example.com`. A secret so it is masked in the public logs |
| `LOLINET_NODES_YAML` | Optional. Your whole `nodes.yaml`, if its `cloudflare.bucket` isn't `lolinet-dist` |

Without the first three, `dist` skips quietly. It doesn't deploy the Worker or the probe page: those need your real node list and `PROBE_MASTER` (sections 3 and 4). To upload a release again, run `dist` by hand with its tag; with no tag it uploads `install.sh`.

By hand, the same thing is:

```sh
gh release download <tag> --repo prnake/lolinet --dir out
uv run join/deploy.py release out        # a lolinet-… release
uv run join/deploy.py lolidesk out       # a lolidesk-… release
```

To roll back, upload the older release again. A device can follow GitHub instead of your join site with `sudo lolinet up --upgrade-source github:<owner>/<repo>`. Devices that were set up with `easytier-core` by an earlier join script switch over with one `sudo lolinet up`.

## Building packages

`make lolinet` (or `sh lolinet/build.sh`) builds for the machine you're on. It clones the pinned EasyTier into `.build/easytier`, applies the patches, and writes `.build/out/lolinet-<os>-<arch>.tar.gz`. You need the Rust toolchain named in EasyTier's `rust-toolchain.toml` and `protoc`. Cross-compile with `TARGET=x86_64-apple-darwin FEATURES=jemalloc sh lolinet/build.sh`.

The join site needs all four platforms: `linux-x86_64`, `linux-aarch64`, `macos-aarch64` and `macos-x86_64`. The easiest way to get them is the [`build`](../.github/workflows/build.yml) workflow on your fork. With the four packages in one directory, `deploy.py release <dir>` writes the manifest itself.

### Releasing from a fork

- **lolinet:** run the `build` workflow on `main` with **release** checked, or push a `v*` tag. It publishes `lolinet-<version>` with the four `lolinet-*` and `easytier-*` packages, the probe bundle, `SHA256SUMS` and `lolinet.json`, and marks it latest.
- **lolidesk:** every push to `main` that changes `desk/` publishes `lolidesk-<commit>` with `lolidesk.json` once the `desk` workflow passes, without marking it latest. To publish by hand, run `desk` on `main` with **release** checked.

Either way, `dist` then copies the release to your join site if the secrets above are set.

Devices that follow GitHub read `releases/latest/download/lolinet.json`, so `latest` must always be a lolinet release. Publish lolinet before your first lolidesk release: when a repository has only one release, GitHub treats it as latest whatever the flag says.
