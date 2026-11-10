# Security

## Reporting a vulnerability

Please report it privately, not in a public issue:

1. Use **[Report a vulnerability](https://github.com/prnake/lolinet/security/advisories/new)** on this repository's Security tab.
2. If that isn't available, open an issue that says only that you have a security report, with no details, and the maintainer will get in touch.

Include what is affected, how to reproduce it, and what an attacker gains. Reports in English or Chinese are both fine.

## Supported versions

Fixes go into the latest `lolinet-*` release. Devices with auto-upgrade on pick it up within 6 hours of it reaching their upgrade source.

## Scope

In scope:

- **`lolinet`**: the CLI, its services, and `lolinet upgrade` (manifest handling, checksum and test run, the atomic switch).
- **`join/`**: the Worker, `install.sh` and pairing-code redemption.
- **`scripts/`**: credential signing, pairing-code encryption, revocation.
- **The public probe**: anything that lets the public page or `/v1/probe` reveal a relay or device name, an address, the SNI, the console's host, or a key. Forging another node's report also counts.
- **The local console's agent**: anything that gets past its loopback binding, `Host` allow-list, `X-Mesh-Agent` header check or read-only mode, or that gets a secret field to the browser.
- **lolidesk**: authentication, certificate pinning, and input injection.
- **`patches/`**: bugs the patches introduce into EasyTier.

Bugs in EasyTier itself belong [upstream](https://github.com/EasyTier/EasyTier/security), unless a patch here causes them.

## What the design protects

[Architecture](docs/architecture.md) explains each mechanism. In short:

- **No shared secret on devices.** Each device has its own credential, valid up to a year and revocable on its own.
- **The join site sees only ciphertext.** Pairing ciphertext is stored under a hash of the code, deleted on first use, and expires after 15 minutes.
- **Packages are checked before they run.** The install script and auto-upgrade check the sha256, and auto-upgrade also test-runs the new binary before switching.
- **Probe keys are per node.** Each probe report is signed with a key derived for that node, so one compromised node can forge only its own card.
- **This repository holds no secrets.** They are generated at deploy time and kept in `$LOLINET_HOME/state/` and Worker secrets.
