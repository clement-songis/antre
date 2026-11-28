# Antre Docs

The published documentation at <https://antre-ai.github.io/docs/> is built with Astro from the Markdown files in `docs/src/`, with a Pagefind full-text index. It is served under the `/docs` base path, so internal links go through `withBase()` (`src/lib/docs.ts`).

## Local development

```bash
cd docs
npm ci
npm run dev
```

## Production build

```bash
cd docs
npm run build
npm run preview
```

`npm run build` writes `docs/dist/`. The Site workflow (`.github/workflows/site.yml`) copies it into the site built by `website/` and publishes both; see `website/README.md`.
