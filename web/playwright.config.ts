import { defineConfig, devices } from "@playwright/test";

const PORT = 8099;
const SCRATCH = ".e2e-tmp";

export default defineConfig({
  testDir: "./e2e",
  testIgnore: ["screenshots.spec.ts", "demo.spec.ts"],
  retries: 0,
  workers: 1,
  use: {
    baseURL: `http://127.0.0.1:${PORT}`,
    trace: "retain-on-failure",
  },
  projects: [{ name: "chromium", use: devices["Desktop Chrome"] }],
  webServer: {
    command:
      `pnpm --dir web build && rm -rf web/${SCRATCH} ` +
      `&& cargo xtask broadcast-fixtures --out web/${SCRATCH}/recordings ` +
      `&& cargo run -q -p sdrmm --no-default-features -- --bind 127.0.0.1:${PORT} ` +
      `--db web/${SCRATCH}/e2e.db --recordings-dir web/${SCRATCH}/recordings`,
    cwd: "..",
    env: { VITE_ENABLE_SYNTHETIC_DEVICES: "true" },
    url: `http://127.0.0.1:${PORT}/api/state`,
    reuseExistingServer: false,
    timeout: 300_000,
  },
});
