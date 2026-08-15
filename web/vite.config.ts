import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

export default defineConfig({
  plugins: [react(), tailwindcss()],
  server: { proxy: { "/api": { target: "http://127.0.0.1:8080", ws: true } } },
  test: { exclude: ["e2e/**", "node_modules/**", "dist/**"] },
});
