import react from "@astrojs/react";
import tailwindcss from "@tailwindcss/vite";
import { defineConfig } from "astro/config";
import { SITE } from "./src/seo.ts";

export default defineConfig({
  site: SITE,
  compressHTML: true,
  devToolbar: { enabled: false },
  build: { format: "file" },
  integrations: [react({ exclude: [/node_modules/], compiler: true })],
  vite: {
    plugins: [tailwindcss()],
    resolve: { dedupe: ["react", "react-dom"] },
    build: { chunkSizeWarningLimit: 1000 },
    server: { fs: { allow: [".."] } },
  },
});
