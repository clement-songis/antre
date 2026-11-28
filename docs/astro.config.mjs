import { defineConfig } from "astro/config"
import mdx from "@astrojs/mdx"
import { mdbookCompat } from "./src/lib/remark-mdbook.mjs"

const copyButtonTransformer = {
  name: "antre-copy-button",
  pre(node) {
    node.properties["data-code"] = this.source
  },
}

// Served under /docs/ of https://antre-ai.github.io, built by the antre-ai/antre-ai.github.io repo.
const siteDomain = process.env.SITE_DOMAIN?.trim()
const site = siteDomain ? `https://${siteDomain}` : "https://antre-ai.github.io"
const base = "/docs"

export default defineConfig({
  site,
  base,
  trailingSlash: "ignore",
  integrations: [mdx()],
  build: {
    format: "file",
  },
  markdown: {
    remarkPlugins: [[mdbookCompat, { base }]],
    shikiConfig: {
      themes: {
        light: "min-light",
        dark: "github-dark",
      },
      transformers: [copyButtonTransformer],
    },
  },
})
