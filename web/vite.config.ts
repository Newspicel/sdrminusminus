import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

// The server owns the API + WebSocket on :8080; Vite proxies both (ws:true upgrades /api/ws)
// so the dev origin matches production's same-origin model (PLAN §10).
export default defineConfig({
  plugins: [react(), tailwindcss()],
  server: {
    proxy: {
      "/api": {
        target: "http://127.0.0.1:8080",
        ws: true,
      },
    },
  },
});
