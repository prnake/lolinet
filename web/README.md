# web/

The local console and the public probe page. Two Vite builds share one set of React components. User docs: [docs/console.md](../docs/console.md).

| Path | What it is |
| :-- | :-- |
| `index.html`, `src/App.tsx`, `src/pages/` | Local console: overview, nodes, credentials, pairing, design specimen. Builds to `dist/`, served by the agent |
| `probe.html`, `src/probe/` | Public probe page for the join site's home page. Builds to `dist-probe/` with `vite.probe.config.ts` |
| `agent/mesh_agent.py` | The console's backend: `easytier-cli` snapshots, a filtered proxy to `easytier-web`, pairing through `scripts/pair.py`. Python standard library only |
| `agent/mesh_manager.py` | The always-on intranet variant for the issuer relay: whole-site sign-in with the console account, a Settings page, and server-side pairing through the join Worker's `/v1/manager/*`. See [docs/console.md](../docs/console.md#manager-on-the-issuer-relay) |
| `vite.nodes.ts` | Reads `nodes.yaml` into `virtual:mesh-nodes`. The public build keeps only what may be published |
| `src/ui/`, `src/viz/`, `src/styles/` | Components, topology / matrix / path charts, and the design tokens |
| `DESIGN.md` | The Catseye design language (Chinese). Every colour, size, radius and duration lives in `src/styles/tokens.css` |
| `scripts/check-probe-bundle.mjs` | Fails if the public bundle contains an internal name, address, SNI, console domain or secret field |

## Scripts

```sh
npm ci
npm run build          # tsc + dist/ (console) + dist-probe/ (public probe)
npm run build:probe    # dist-probe/ only
npm run dev            # console on http://127.0.0.1:5188, /api proxied to the agent
npm run dev:probe      # probe on http://127.0.0.1:5189/probe.html, /v1 proxied to the join site
npm run agent          # python3 agent/mesh_agent.py
node scripts/check-probe-bundle.mjs   # after a build; CI runs it too
```

## Dev loop

1. Start the agent against a running kernel, e.g. `python3 agent/mesh_agent.py` (TUN service) or `--rpc 127.0.0.1:15897 --socks 127.0.0.1:18080` (no-tun).
2. `npm run dev`. Set `MESH_AGENT=http://…` if the agent isn't on `127.0.0.1:10188`.
3. No kernel at hand? Open the page with `?demo`.

For the probe, `npm run dev:probe` proxies `/v1` to `LOLINET_BASE_URL` (or `https://<network.join>`); `MESH_JOIN` overrides it. `?demo` works there too.

Builds read `LOLINET_NODES`, else `../nodes.yaml`, else `../nodes.example.yaml`. CI ([web.yml](../.github/workflows/web.yml)) builds against `nodes.example.yaml` and runs the bundle check.

When you change a token, look at the `#/design` page before anything else. Components don't hard-code colours, font sizes, radii or durations.
