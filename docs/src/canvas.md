# Design Canvas

The design canvas lets the agent build visual designs (mockups, landing
pages, UI screens, slides, one-pagers) as plain HTML, shows them in a
sandboxed live preview, keeps every version, and exports or publishes them.

This page covers the backend: storage, agent tools, the render origin, the
API and the contract the web UI preview panel uses.

## Designs and versions

A design is a directory:

```
~/.moltis/designs/<id>/
├── index.html      # entry document
├── tokens.css      # copied from the design system, if one was picked
└── ...             # any CSS, JS, images or fonts the agent adds
```

`<id>` is 32 lowercase hex characters. Metadata (title, session, project,
design system, current version) lives in `moltis.db`
(`canvas_designs`), and every version in `canvas_design_versions`.

Versions are **append-only**: a database trigger refuses updates, and a
revert restores an old snapshot and records it as a *new* version
(`revert to v2`). Each version is a checkpoint of the whole directory in
`~/.moltis/design-versions/` (the same mechanism as
[Checkpoints](checkpoints.md), in a separate folder that is never pruned).
A version is only recorded when the files changed (content hash).

Versions are taken:

- when a design is created (`created`),
- at the end of every agent turn that touched the design (`agent turn`): a
  hook records which designs `Write`, `Edit` and `MultiEdit` targeted, and
  which ones `design_create` / `design_open` returned,
- before publishing the live files (`publish`),
- on revert (`revert to vN`).

Designs may only contain regular files and folders. A symlink inside a
design stops versioning with an explicit error until it is removed.

## Agent tools

| Tool | Purpose |
|---|---|
| `design_create` | Create a design (title, optional initial `html`, optional `design_system`). Returns its directory, entry file, the design system's `DESIGN.md`, the vendored scripts and the design rules. |
| `design_open` | Open an existing design: directory, files, recent versions, design system guidance. |
| `design_screenshot` | Render the current files (or a past `version`) to PNG at a given width, returned as an image so the agent can check its work. |

There is no separate edit tool: the agent edits files with the regular
[filesystem tools](tools-fs.md) using the absolute paths returned above.
Writes into `~/.moltis/designs/` that are not inside an existing design
are refused.

Designs follow these rules (returned to the agent with every create/open):

- no network: no `fetch`, XHR, WebSocket, remote images, fonts or scripts;
- no forms, no navigation of the top window;
- shared libraries only from the vendored set at `/_vendor/<name>`;
- `data-mid="<stable-id>"` on every semantic block, kept stable across
  edits, so selections and comments survive.

## Design systems

A design system is a [skill](skill-tools.md) folder with a `DESIGN.md`
(guidance) and a `tokens.css` (CSS custom properties). Three neutral starters
are seeded into `~/.moltis/skills/` on startup and never overwritten:

| Name | Look |
|---|---|
| `minimal` | Grayscale, one blue accent, product UI and dashboards |
| `editorial` | Paper tones, serif headlines, articles and reports |
| `midnight` | Dark layers, violet accent, developer tools and launch pages |

`design_create(design_system: "minimal")` copies `tokens.css` into the
design, links it from `index.html` and returns `DESIGN.md` to the agent.
Any skill folder containing both files can be used by its folder name
(`design-system-<name>` folders are also found by `<name>`).

## Render origin

Designs are never served by the app. A second listener, the **render
origin**, serves them on its own port (or a separate hostname behind a
proxy). It has no API, reads no cookies and sets none.

| Path | Content |
|---|---|
| `/d/<token>/<file>` | A design file. `<token>` is an HMAC-signed link naming the design, the version (or the live files) and an expiry. |
| `/p/<share-id>?k=<key>` | A published design, wrapped in a notice banner and a sandboxed iframe. |
| `/p/<share-id>/content?k=<key>` | The published single-file HTML (only frameable by the wrapper). |
| `/_vendor/<name>` | Vendored scripts. |
| `/healthz` | `ok` |

Every HTML response carries this policy (origin shown as
`http://127.0.0.1:18790`):

```
sandbox allow-scripts;
default-src 'none';
script-src 'unsafe-inline' http://127.0.0.1:18790/_vendor/ http://127.0.0.1:18790/d/<token>/;
style-src 'unsafe-inline' http://127.0.0.1:18790;
img-src http://127.0.0.1:18790 data: blob:;
font-src http://127.0.0.1:18790 data:;
media-src http://127.0.0.1:18790 data: blob:;
connect-src 'none'; worker-src 'none'; frame-src 'none'; child-src 'none';
manifest-src 'none'; object-src 'none'; base-uri 'none'; form-action 'none';
frame-ancestors *
```

- The CSP `sandbox` directive (no `allow-same-origin`, `allow-forms`,
  `allow-popups` or `allow-top-navigation`) puts the page in an opaque
  origin even when opened directly, so it cannot read cookies or call the
  app with them.
- `connect-src 'none'` and the restricted `img-src`/`font-src`/`style-src`
  stop exfiltration through requests, images, fonts or CSS.
- Other headers: `X-Content-Type-Options: nosniff`,
  `Referrer-Policy: no-referrer`, `Cache-Control: no-store`,
  `Cross-Origin-Resource-Policy: cross-origin` (opaque-origin pages loading
  their own assets are cross-origin), a restrictive `Permissions-Policy`,
  `X-Robots-Tag: noindex`.
- Request paths are checked component by component (no `..`, no absolute
  paths, no hidden segments) and the resolved file must stay inside the
  design directory after symlinks are resolved.
- Preview links expire after `canvas.token_ttl_secs`; the signing key is
  random per process, so links also die on restart.

Vendored scripts (Preact and Preact hooks) are compiled into the binary.
Operators can add `.js`/`.css` files to `~/.moltis/canvas/vendor/`; they are
served at `/_vendor/<name>` and listed with their SRI hash by
`canvas.config`. Extra CDN origins are opt-in through
`canvas.script_sources`.

```admonish warning title="TLS"
The render origin listens over plain HTTP. Browsers allow an
`http://localhost` or `http://127.0.0.1` iframe inside an HTTPS page, but
not other hosts. When the app is served over HTTPS on another host, put the
render origin behind the same TLS proxy (its own hostname or port) and set
`canvas.public_url`.
```

## Exports and publishing

| Format | How |
|---|---|
| HTML | Single file: local CSS, scripts, images, fonts and vendored files inlined. |
| ZIP | Every file of the design under a `<title>/` folder. |
| PNG / PDF | The single-file HTML rendered by headless Chrome (`Page.captureScreenshot` / `Page.printToPDF`) with an offline CSP `<meta>` tag, in a throwaway browser session separate from the browser tool. |

Exports are downloaded as attachments with `Content-Security-Policy:
sandbox`, so generated HTML never runs on the app origin.

Publishing freezes one version into an immutable single-file snapshot in
the same share store as session shares (private by default, with an access
key; one active link per design; revocable). Published pages are served only
by the render origin, framed by a banner warning that the content is
AI-generated.

## Configuration

```toml
[canvas]
enabled = true                    # Design tools, canvas.* API, render origin
# bind = "127.0.0.1"              # Default: server.bind
# port = 18790                    # Default: a free port picked at startup
# public_url = "https://designs.example.com"
token_ttl_secs = 3600             # Preview link lifetime (60 s to 7 days)
script_sources = []               # Extra https:// origins for script-src
allow_eval = false                # Allow eval/new Function (in-browser JSX)
max_file_bytes = 10485760         # Largest file served or exported
render_sandbox = false            # Render PNG/PDF in a browser container
render_timeout_secs = 30
seed_design_systems = true
```

The feature is compiled in by default (`canvas` Cargo feature).

## API

RPC (`canvas.*`) and REST (`/api/canvas/...`) need the usual app
authentication. Reads need `operator.read`, writes `operator.write`.

| RPC | REST | Scope |
|---|---|---|
| `canvas.config {host?}` | `GET /api/canvas/config` | read |
| `canvas.list {sessionKey?, projectId?, limit?}` | `GET /api/canvas/designs` | read |
| `canvas.get {designId}` | `GET /api/canvas/designs/{id}` | read |
| `canvas.versions {designId, limit?}` | `GET /api/canvas/designs/{id}/versions` | read |
| `canvas.preview_url {designId, version?, host?}` | `GET /api/canvas/designs/{id}/preview-url?version=` | read |
| `canvas.design_systems` | (in `config`) | read |
| — | `GET /api/canvas/designs/{id}/export/{html,zip,png,pdf}?version=&width=&height=&fullPage=` | read |
| `canvas.revert {designId, version}` | `POST /api/canvas/designs/{id}/revert` | write |
| `canvas.rename {designId, title}` | — | write |
| `canvas.publish {designId, version?, visibility?, host?}` | `POST /api/canvas/designs/{id}/publish` | write |
| `canvas.unpublish {shareId}` | `DELETE /api/canvas/shares/{shareId}` | write |

`host` is the web UI's `location.host`. Without `canvas.public_url`,
preview links reuse that host name with the render port, so they work on
loopback and on the LAN alike.

### Events

Pushed on the WebSocket event bus:

| Event | Payload |
|---|---|
| `canvas.design.created` | `{design}` |
| `canvas.version.created` | `{designId, version, revertedFrom?}` |
| `canvas.design.published` | `{designId, shareId, version, visibility}` |
| `canvas.design.unpublished` | `{designId, shareId}` |

## Preview panel contract (for the web UI)

The preview panel and the element-pick bridge are built in the web UI.
This is the contract they rely on.

### Framing

```html
<iframe
  src="{previewLink.url}#host={encodeURIComponent(location.origin)}"
  sandbox="{previewLink.sandbox}"   <!-- "allow-scripts", never add allow-same-origin -->
  referrerpolicy="no-referrer"
  allow=""
></iframe>
```

- Get the link from `canvas.preview_url` (pass `version` for the version
  picker) and refresh it before `expiresAt`.
- Reload the frame on `canvas.version.created` for the shown design.
- Viewport toggles only resize the iframe (390 / 768 / 1280 px wide).

### postMessage protocol `antre.canvas/1`

Every message is an object `{protocol: "antre.canvas/1", type, ...}`.
Anything else is ignored.

Frame → host:

| `type` | Fields |
|---|---|
| `ready` | `title: string`, `mids: string[]` (every `data-mid` on the page, max 500) |
| `pick.result` | `element: {mid?: string, selector: string, tag: string, text: string, html: string, rect: {x, y, width, height}}` |
| `pick.cancel` | — |
| `resize` | `height: number` (document height, for auto-height frames) |
| `error` | `message: string`, `source?: string`, `line?: number` (uncaught page errors, to feed back to the agent) |

Host → frame:

| `type` | Fields |
|---|---|
| `pick.start` | — (hover highlight on, next click reports `pick.result`) |
| `pick.stop` | — |
| `highlight` | `mid?: string`, `selector?: string` (outline one element; neither clears) |

Validation rules:

- **Host side.** Accept a message only if `event.source ===
  iframe.contentWindow`. The sandboxed frame's origin is opaque, so
  `event.origin` is the string `"null"`; check that too, but the source
  check is what matters. Validate the shape strictly, cap strings (`text`
  500, `html` 300, `selector` 500, `message` 1000 characters), and render
  every field as text, never as HTML.
- **Frame side.** The bridge reads the expected host origin from
  `location.hash` (`#host=`) and accepts messages only when
  `event.source === window.parent` and `event.origin` equals it. Replies go
  to `window.parent.postMessage(message, hostOrigin)`.
- No secrets cross the boundary in either direction; the frame never
  receives tokens, session data or file paths.
- The bridge never evaluates message content.

The bridge script itself will ship as a vendored file
(`/_vendor/canvas-bridge.js`, already allowed by `script-src`) that the
render origin injects into HTML responses of preview links. That injection
is part of the UI work.
