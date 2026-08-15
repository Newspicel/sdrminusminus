import { defineConfig, devices } from "@playwright/test";

const PORT = 8098;
const SCRATCH = ".shots-tmp";

export default defineConfig({
  testDir: "./e2e",
  testMatch: "screenshots.spec.ts",
  retries: 0,
  workers: 1,
  timeout: 240_000,
  use: {
    ...devices["Desktop Chrome"],
    baseURL: `http://127.0.0.1:${PORT}`,
    viewport: { width: 1440, height: 900 },
    deviceScaleFactor: 1.5,
    colorScheme: "dark",
    trace: "retain-on-failure",
  },
  projects: [{ name: "chromium" }],
  webServer: {
    command:
      `rm -rf web/${SCRATCH} && mkdir -p web/${SCRATCH}/recordings ` +
      `&& cp fixtures/*.sigmf-meta fixtures/*.sigmf-data web/${SCRATCH}/recordings/ ` +
      `&& cargo run -q -p sdrmm -- --bind 127.0.0.1:${PORT} ` +
      `--db web/${SCRATCH}/shots.db --recordings-dir web/${SCRATCH}/recordings ` +
      `--playback-speed 20`,
    cwd: "..",
    url: `http://127.0.0.1:${PORT}/api/state`,
    reuseExistingServer: false,
    timeout: 300_000,
  },
});
