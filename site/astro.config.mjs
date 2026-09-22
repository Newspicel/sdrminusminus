import react from "@astrojs/react";
import tailwindcss from "@tailwindcss/vite";
import { defineConfig } from "astro/config";

export default defineConfig({
  site: "https://sdrmm.newspicel.dev",
  compressHTML: true,
  devToolbar: { enabled: false },
  build: { format: "file" },
  integrations: [
    react({
      exclude: [/node_modules/],
      babel: { plugins: ["babel-plugin-react-compiler"] },
    }),
  ],
  vite: {
    plugins: [tailwindcss()],
    resolve: { dedupe: ["react", "react-dom"] },
    build: { chunkSizeWarningLimit: 1000 },
    server: { fs: { allow: [".."] } },
  },
});
