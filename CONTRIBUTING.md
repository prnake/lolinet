# Contributing

Thanks for helping. Bug reports, questions and pull requests are welcome, in English or Chinese.

- **A bug or a question:** open an [issue](https://github.com/prnake/lolinet/issues/new/choose). Include `lolinet --version`, your OS and architecture, and what `lolinet status` shows, with names and addresses removed.
- **A security problem:** don't open an issue; see [SECURITY.md](SECURITY.md).
- **A larger change:** open an issue first, so we can agree on the approach before you write it.

## Set up

| Tool | For |
| :-- | :-- |
| [uv](https://docs.astral.sh/uv/) | The admin scripts (`uv run …`; each script declares its own dependencies) |
| Node 24 and npm | `web/` |
| Rust (stable) | `desk/`. On macOS, Xcode 26 or later |
| Rust (EasyTier's `rust-toolchain.toml`) and `protoc` | Building `lolinet` itself |

Nothing needs a real network: the web build uses `nodes.example.yaml`, and the console and probe pages have a demo mode (`?demo`).

## Build and test

```sh
make check          # web build + public-bundle leak check, Worker / shell / Python syntax, desk fmt + clippy + tests
make lolinet        # the real lolinet build: clones EasyTier into .build/, applies the patches (takes a while)
```

CI runs three workflows on every pull request, each when its files change:

| Workflow | Checks |
| :-- | :-- |
| [`build`](.github/workflows/build.yml) | EasyTier and `lolinet` for four platforms; static linking and size limits; the SNI check on every binary the runner can execute |
| [`desk`](.github/workflows/desk.yml) | lolidesk fmt, clippy, tests and builds for three targets; a terminal session over loopback |
| [`web`](.github/workflows/web.yml) | The console and probe builds; no internal names in the public bundle; Worker, shell and Python syntax |

To test the SNI check by hand after `make lolinet`:

```sh
ln -sf lolinet .build/pkg/easytier-core && make check-sni CORE=.build/pkg/easytier-core
```

## Pull requests

- One topic per pull request. Say what changed, why, and how you tested it.
- All three workflows must pass. Nothing is pushed to `main` directly.
- Update the docs with the code: [docs/reference.md](docs/reference.md) for commands, flags, files, settings and endpoints; [docs/architecture.md](docs/architecture.md) for how things work. `README.md` and `README.zh-CN.md` change together.
- Comments, CLI output and UI text are written in Chinese. Match the code around you where you can; English is fine for a first contribution.

Some things can't change without a migration path, because devices in the field depend on them: installed paths, service names and launchd labels, the release package format and manifest, the pairing crypto, and the probe's key derivations. [AGENTS.md](AGENTS.md) lists them, along with the places that must change together.

## License

By contributing you agree that your contribution is licensed under [LGPL-3.0](LICENSE), the license of this repository.
