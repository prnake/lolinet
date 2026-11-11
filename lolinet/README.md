# lolinet/

Source of the `lolinet` CLI: EasyTier and a small command line in one binary that dispatches on `argv[0]`. Usage: [reference](../docs/reference.md#lolinet-cli). Design: [architecture](../docs/architecture.md#the-lolinet-binary).

| Path | What it is |
| :-- | :-- |
| `src/main.rs` | Entry point, `argv[0]` dispatch, the clap commands, `up` / `down` / `run`, taking over older installs |
| `src/service.rs` | The launchd and systemd services and the upgrade job |
| `src/status.rs` | `lolinet status`, from the built-in `easytier-cli`'s JSON (the same data the console's agent and the probe read) |
| `src/upgrade.rs` | `lolinet upgrade`: manifest sources, download, sha256, test run, atomic switch; an installed lolidesk too |
| `src/probe.rs` | `lolinet probe`: the device's public probe (hardware and location, signed like `probe/reporter.py`'s device mode), and taking over the older Python one |
| `src/netcfg.rs` | Reading the overlay address and mode from `loli.toml`, and switching between TUN and userspace by editing two lines in place |
| `src/invite.rs` | `lolinet invite`: ask the manager for a pairing code over the overlay (through the local SOCKS5 in userspace mode) |
| `src/exit.rs` | `lolinet exit`: the exit node's SOCKS5 server (`CONNECT`, `UDP ASSOCIATE`), its source and destination rules, and its settings |
| `build.sh` | Local build: clone the pinned EasyTier, apply the patches, copy `src/` in, build, package |
| `release.py` | Writes the release manifest `lolinet.json`, or with `--lolidesk` the lolidesk manifest `lolidesk.json` (standard library only) |
| `check_exit.py` | End-to-end test of the exit node without root: destination refusals, `CONNECT` and UDP DNS to the internet, the source allow-list, and two userspace nodes where one uses the other's exit across the overlay |
| `check_invite.py` | End-to-end test of member invitations on loopback: an issuer node, a member, a stand-in join-site Worker and nginx; `lolinet invite` gets a code, a new node joins with it, and the manager's policy refuses what it should |
| `check_probe.py` | End-to-end test of the built-in probe: install as root, drop to `nobody`, take over the Python probe, change id, upgrade, turn off, and verify reports the way the join site does |

## Build

This source compiles only inside the EasyTier tree, as the third binary of the `easytier` crate (`patches/easytier-v2.7.0-lolinet.patch` adds the `[[bin]]`). `build.sh` sets that up:

```sh
sh lolinet/build.sh                                        # this machine → .build/out/lolinet-<os>-<arch>.tar.gz
TARGET=x86_64-apple-darwin FEATURES=jemalloc sh lolinet/build.sh   # cross-compile
```

- It clones [`easytier/build.env`](../easytier/build.env)'s tag into `.build/easytier` once, checks the commit, then resets the tree and applies `EASYTIER_PATCHES` and `LOLINET_PATCHES` on every run. `target/` is kept for incremental builds.
- You need the Rust toolchain from EasyTier's `rust-toolchain.toml` and `protoc`.
- `LOLINET_VERSION` comes from `git describe --always --dirty`; `lolinet --version` prints it, and the upgrade check compares it with the manifest.
- `aarch64` builds use `mimalloc`, `x86_64` builds `jemalloc`, the same as CI.

CI ([build.yml](../.github/workflows/build.yml)) builds four platforms with `easytier-core` and `easytier-cli`, checks that Linux binaries are static and that `lolinet` stays under 32 MiB (14 MiB packaged), then runs `easytier/check_sni.py` against `easytier-core` and against `lolinet` linked as `easytier-core`, `easytier/check_udp_pref.py` against `lolinet`, `lolinet/check_exit.py` and `lolinet/check_invite.py`. On the two platforms it can run natively it also runs `sudo lolinet/check_probe.py`, which installs the probe for real.

## Release

```sh
python3 lolinet/release.py <dir with the four lolinet-*.tar.gz>   # writes lolinet.json
```

The version is the first line of each package's `BUILD-INFO` (`lolinet <version>`); all four must agree. The `build` workflow does this when it publishes a release; `join/deploy.py release <dir>` checks it again before uploading to the join site.

## Not done yet

- `lolinet join <code>`: redeem and decrypt inside `lolinet`, so `install.sh` only downloads.
- The relay mode of the probe (load, links) inside `lolinet`, once relays run it. Then `probe/reporter.py` can go.
- GUI clients that drive `lolinet` (`lolinet cli -o json …`) instead of managing their own kernel process.
