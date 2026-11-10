# Notes for coding agents

lolinet is a self-hosted mesh network on EasyTier: a Rust CLI built inside a patched EasyTier tree (`lolinet/`, `patches/`, `easytier/`), a Cloudflare Worker join site (`join/`), Python admin scripts (`scripts/`, `probe/`), a React console and public probe page (`web/`), and a separate remote-desktop app (`desk/`). Start with [README.md](README.md), then [docs/architecture.md](docs/architecture.md).

## Build and check

| | |
| :-- | :-- |
| `make check` | Everything CI checks that runs without the EasyTier tree: web build and public-bundle leak check, Worker / shell / Python syntax, desk fmt + clippy + tests |
| `make check-web`, `make check-scripts`, `make check-desk` | One part of the above |
| `make lolinet` | The real `lolinet` build (clones EasyTier into `.build/`, several minutes). `lolinet/src` does not compile on its own |
| `make check-sni CORE=…` | Prove the SNI patch on a built `easytier-core` or `lolinet` linked under that name |

Run what covers your change before saying it works, and say what you didn't run.

## Invariants

Devices in the field depend on these. Changing one needs a migration path, not just an edit.

- **No secrets, no real node list.** `nodes.yaml`, `.env` and `state/` are git-ignored; secrets are generated at deploy time into `$LOLINET_HOME/state/` (mode 600) and Worker secrets. Examples use `example.com` domains and `203.0.113.0/24` addresses.
- **Installed paths and names are stable**: `/usr/local/bin/lolinet`, `/etc/lolinet/`, `/var/log/lolinet/`, `/etc/mesh-probe/`, the launchd labels `co.lo-li.*`, the systemd units `lolinet.service`, `lolinet-upgrade.{service,timer}`, `lolinet-probe.service`, `mesh-probe.service` (relays), and the legacy paths `lolinet up` takes over (`/etc/easytier/loli.toml`, `/usr/local/libexec/easytier/`, `/opt/easytier/`, `Application Support/lo-li mesh/probe`, the device-mode `/etc/mesh-probe/`).
- **Release format is a contract** with `lolinet upgrade` and `install.sh`: package names `lolinet-<macos|linux>-<aarch64|x86_64>.tar.gz` with the binary at the archive root, `BUILD-INFO`'s first line `lolinet <version>`, the `lolinet.json` schema, packages next to the manifest. `DEFAULT_GITHUB` in `lolinet/src/upgrade.rs` is the fallback source.
- **`releases/latest` is always a lolinet release.** Devices on `github:` sources fetch `releases/latest/download/lolinet.json`. lolidesk releases use `--latest=false`.
- **The public probe names nothing.** No relay or device names, overlay or public addresses, SNI, console host or secret fields in `web/dist-probe/` or `/v1/probe`. `meshNodes({ public: true })` in `web/vite.nodes.ts` and `web/scripts/check-probe-bundle.mjs` enforce it; the probe build must not import from `web/src/data/store`.
- **`lolinet` stays small and static.** CI fails a binary over 32 MiB, a package over 14 MiB, or a non-static Linux build. GUI or system-framework code goes in `desk/`.
- **The agent's guard rails** (`web/agent/mesh_agent.py`): loopback by default, `Host` allow-list, `X-Mesh-Agent` on writes, read-only on non-loopback listens, secret fields stripped from console responses.

## Keep in sync

| What | Where |
| :-- | :-- |
| Pairing crypto (PBKDF2-SHA256, 200,000 rounds, AES-256-CBC, `sha256(code)[:32]` as the KV key) | `scripts/pair.py`, `join/install.sh` |
| Probe key derivations (`probe:<name>`, `devid`) and the signature format | `join/worker.js`, `probe/reporter.py`, `probe/device.py`, `lolinet/src/probe.rs`, `join/deploy.py` |
| The four platforms | `.github/workflows/build.yml`, `lolinet/release.py`, `join/deploy.py`, `lolinet/build.sh`, `join/install.sh` |
| Probe bundle file list | `lolinet/release.py` (`PROBE_FILES`), `join/install.sh`, `lolinet/src/upgrade.rs` |
| Node list lookup (`LOLINET_NODES` → `nodes.yaml` → `nodes.example.yaml`) | `join/deploy.py`, `scripts/device_config.py`, `probe/device.py`, `web/vite.nodes.ts`, `web/scripts/check-probe-bundle.mjs` |
| EasyTier version | `easytier/build.env`, patch file names, `nodes.example.yaml` (`network.easytier`), README badges |
| Design tokens | `web/src/styles/tokens.css` is the only source; `web/DESIGN.md` documents it; `#/design` shows it |

## Upgrading EasyTier

Bump `EASYTIER_REF` and `EASYTIER_SHA` in `easytier/build.env`, rebase every patch in `patches/` onto the new tag (rename the files to the new version), and update the version everywhere listed above. CI checks that the tag still points at the SHA, applies the patches, and runs the SNI check (`easytier/check_sni.py`) and the tunnel-preference check (`easytier/check_udp_pref.py`).

## Conventions

- **Language.** Code comments, CLI output, UI text and commit messages are Chinese, in short plain sentences; keep it that way. User docs (`README.md`, `docs/`, component READMEs) are English. `README.zh-CN.md` mirrors `README.md`: change both together. `web/DESIGN.md` is Chinese.
- **Docs follow code.** A change to a command, flag, file, setting or endpoint updates [docs/reference.md](docs/reference.md); a change to how something works updates [docs/architecture.md](docs/architecture.md).
- **Pull requests only.** Nothing is pushed to `main` directly; `build`, `desk` and `web` must pass.
- **Python** scripts are run with `uv run` and declare their dependencies inline (PEP 723). `probe/reporter.py`, `easytier/check_sni.py`, `lolinet/release.py` and `web/agent/` use only the standard library; the reporter and the agent must keep running on macOS's Python 3.9.
