import { defineConfig, devices } from "@playwright/test";

const PORT = 8099;
const SCRATCH = ".e2e-tmp";

export default defineConfig({
  testDir: "./e2e",
  testIgnore: "screenshots.spec.ts",
  retries: 0,
  workers: 1,
  use: {
    baseURL: `http://127.0.0.1:${PORT}`,
    trace: "retain-on-failure",
  },
  projects: [
    {
      name: "chromium",
      use: devices["Desktop Chrome"],
      testIgnore: ["screenshots.spec.ts", "field.spec.ts"],
    },
    { name: "mobile", use: devices["Pixel 7"], testMatch: "field.spec.ts" },
  ],
  webServer: {
    // The virtual radios the specs bind to are gated by a build-time flag, and the server embeds
    // whatever `web/dist` holds, so the UI has to be built here rather than by the caller.
    command:
      `pnpm --dir web build && rm -rf web/${SCRATCH} ` +
      `&& cargo run -q -p sdrmm --no-default-features -- --bind 127.0.0.1:${PORT} ` +
      `--db web/${SCRATCH}/e2e.db --recordings-dir web/${SCRATCH}/recordings`,
    cwd: "..",
    env: { VITE_ENABLE_SYNTHETIC_DEVICES: "true" },
    url: `http://127.0.0.1:${PORT}/api/state`,
    reuseExistingServer: false,
    timeout: 300_000,
  },
});
