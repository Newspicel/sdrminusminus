import babel from "@rolldown/plugin-babel";
import tailwindcss from "@tailwindcss/vite";
import react, { reactCompilerPreset } from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

export default defineConfig({
  plugins: [react(), babel({ presets: [reactCompilerPreset()] }), tailwindcss()],
  server: { proxy: { "/api": { target: "http://127.0.0.1:8080", ws: true } } },
  test: { exclude: ["e2e/**", "node_modules/**", "dist/**"] },
});
