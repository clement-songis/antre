import preact from "@preact/preset-vite";
import { resolve } from "path";
import { defineConfig } from "vite";

export default defineConfig({
  plugins: [preact()],
  root: ".",
  resolve: {
    alias: {
      "@": resolve(__dirname, "src"),
    },
  },
  // The gateway serves the bundle under an asset prefix, not at "/": resolve
  // lazily loaded assets (KaTeX's stylesheet and fonts) relative to the chunk
  // that loads them.
  experimental: {
    renderBuiltUrl: () => ({ relative: true }),
  },
  build: {
    outDir: resolve(__dirname, "../src/assets/dist"),
    emptyOutDir: true,
    sourcemap: false,
    minify: false,
    rollupOptions: {
      input: {
        main: resolve(__dirname, "src/app.tsx"),
        login: resolve(__dirname, "src/login-app.tsx"),
        onboarding: resolve(__dirname, "src/onboarding-app.tsx"),
      },
      output: {
        entryFileNames: "[name].js",
        chunkFileNames: "chunks/[name].js",
        assetFileNames: "[name][extname]",
      },
    },
  },
});
