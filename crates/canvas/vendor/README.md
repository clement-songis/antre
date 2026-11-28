# Vendored scripts for the canvas render origin

Served at `/_vendor/<name>` by the render origin so designs work offline
without any third-party CDN. Each file is pinned and listed in
`src/vendor.rs` with its SHA-384 (SRI) hash.

| File | Upstream | Version | License |
|---|---|---|---|
| `preact.js` | `preact/dist/preact.min.umd.js` | 10.29.1 | MIT (`LICENSE-preact`) |
| `preact-hooks.js` | `preact/hooks/dist/hooks.umd.js` | 10.29.1 | MIT (`LICENSE-preact`) |

The source-map comment is stripped. To update, copy the files from the same
npm package version used by `crates/web/ui` and refresh the hashes with
`cargo test -p antre-canvas vendor` (the test prints the expected values).
