import babel from "@rolldown/plugin-babel";
import tailwindcss from "@tailwindcss/vite";
import react, { reactCompilerPreset } from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

export default defineConfig({
  plugins: [react(), babel({ presets: [reactCompilerPreset()] }), tailwindcss()],
  server: { proxy: { "/api": { target: "http://127.0.0.1:8080", ws: true } } },
  build: {
    chunkSizeWarningLimit: 1000,
    rolldownOptions: {
      output: {
        codeSplitting: {
          groups: [
            { name: "maplibre", test: /node_modules\/(maplibre-gl|pmtiles)\// },
            { name: "flow", test: /node_modules\/@xyflow\// },
            { name: "base-ui", test: /node_modules\/@base-ui\// },
            { name: "react", test: /node_modules\/(react|react-dom|scheduler)\// },
          ],
        },
      },
    },
  },
  test: {
    exclude: ["e2e/**", "node_modules/**", "dist/**"],
    setupFiles: ["./vitest.setup.ts"],
  },
});
