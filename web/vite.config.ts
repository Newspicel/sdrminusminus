import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: { alias: { "@": "/src" } },
  server: { proxy: { "/api": { target: "http://127.0.0.1:8080", ws: true } } },
  // The browser flow is Playwright's (`cargo xtask smoke`); vitest's default glob would pick up
  // its specs and fail on the `test.describe` that only Playwright provides.
  test: { exclude: ["e2e/**", "node_modules/**", "dist/**"] },
});
