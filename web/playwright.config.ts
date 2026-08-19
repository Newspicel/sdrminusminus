import { defineConfig, devices } from "@playwright/test";

const PORT = 8099;
const SCRATCH = ".e2e-tmp";

export default defineConfig({
  testDir: "./e2e",
  testIgnore: "screenshots.spec.ts",
  retries: 0,
  use: {
    baseURL: `http://127.0.0.1:${PORT}`,
    trace: "retain-on-failure",
  },
  projects: [
    { name: "chromium", use: devices["Desktop Chrome"], testIgnore: "field.spec.ts" },
    { name: "mobile", use: devices["Pixel 7"], testMatch: "field.spec.ts" },
  ],
  webServer: {
    command: `rm -rf web/${SCRATCH} && cargo run -q -p sdrmm -- --bind 127.0.0.1:${PORT} --db web/${SCRATCH}/e2e.db --recordings-dir web/${SCRATCH}/recordings`,
    cwd: "..",
    url: `http://127.0.0.1:${PORT}/api/state`,
    reuseExistingServer: false,
    timeout: 180_000,
  },
});
