# Pictesa web UI

The studio UI that the `pictesa` binary serves from `web/dist`: Create (image, video and audio
when the server has audio models), Recipes, Library with a detail viewer, Settings (providers,
agents, billing, about) and, in hosted mode, log in and sign up.

Vite, React 19, strict TypeScript and Tailwind CSS 4. No UI kit, no router or state library:
the app is about 75 KB gzipped for the shell plus a lazy chunk per screen.

Every control comes from the server. Models, params, input slots, presets and recipes are
rendered from their specs (docs/API.md), so a new model needs no UI change.

## Run

Node 22.12 or newer and pnpm.

```sh
pnpm install
pnpm mock           # dev server with the built-in fake API, local mode
pnpm mock:hosted    # the same with accounts and a wallet
VITE_MOCK=token pnpm dev   # local mode that asks for an access token
pnpm dev            # dev server that proxies to a running pictesa server
pnpm build          # production build into web/dist
pnpm typecheck      # app and dev tooling
```

`pnpm dev` proxies `/api`, `/files`, `/thumbs` and `/mcp` to `http://127.0.0.1:4747`
(set `PICTESA_SERVER` for another address, `PORT` for another dev port). The server refuses
foreign hosts and cross-site writes, so start it with the dev origin allowed:

```sh
PICTESA_ALLOWED_HOSTS=localhost:5173,127.0.0.1:5173 pictesa serve
```

To try a production build without the embed step: `pictesa serve --web-dir web/dist`.

## The mock API

`VITE_MOCK` swaps the proxy for a fake server inside the Vite dev server (`mock/`). The browser
code stays the production code: real HTTP, server-sent events, multipart uploads and range
requests. It never reaches `dist`. Data resets on restart.

- About 90 seeded assets, 22 models across 7 providers, presets, 6 recipes and live jobs.
- A prompt with "fail" fails half way; Improve prompt fails when the text contains "error".
- stable-diffusion.cpp is unreachable, Google, Runway and Replicate are not set up.
- Seedream 4 and Wan 2.5 are listed without params, like discovered gateway models.
- Hosted: any email with an 8 character password logs in ("wrongpassword" fails); the
  checkout adds the amount and returns to Settings, Billing.
- Token mode: any token except "wrong" unlocks.

Sample clips and posters in `mock/media` are synthetic (ffmpeg test sources).

## Serving contract

- Serve `index.html` for every GET that is not a file and not under `/api`, `/files`,
  `/thumbs` or `/mcp`. UI paths: `/`, `/recipes`, `/library`, `/settings`,
  `/settings/agents|billing|about|account`, `/login`, `/signup`, each with optional
  `?asset=<id>` for the viewer.
- `/assets/*` names are content hashed and can be cached forever; `index.html` must not be.
- Stripe should return to `/settings/billing?checkout=success` (or `=canceled`).

## Icons

Icons are a subset of Material Symbols Outlined (about 11 KB, FILL axis kept). Add the name to
`src/ui/icon-names.ts`, then run `pnpm icons` (Python with `fonttools` and `brotli`).
`pnpm build` fails when the list and the font disagree.

## Layout

```
src/lib     API client, SSE, stores (jobs, runs, composer draft, catalogs), formatting
src/ui      icon, popover control, dialog, virtualized grid, cards, notices, code blocks
src/pages   create, recipes, library, viewer, settings, auth (each a lazy chunk)
mock        dev-only fake API and its sample media
scripts     icon subset builder and check
```

Design rules: `pictesa.com-nextjs/DESIGN.md` (paper, stone, ink, orange; Poppins and Material
Symbols; 44 px targets; popovers on one control line).

## Licence

AGPL-3.0-only, like the rest of Pictesa.
