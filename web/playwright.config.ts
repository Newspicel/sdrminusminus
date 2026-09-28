import { defineConfig, devices } from "@playwright/test";

declare const process: { readonly env: Readonly<Record<string, string | undefined>> };

const PORT = Number(process.env.E2E_PORT ?? 8099);
const TLS = process.env.E2E_TLS === "1";
const ORIGIN = `${TLS ? "https" : "http"}://127.0.0.1:${PORT}`;
const SCRATCH = ".e2e-tmp";

export default defineConfig({
  testDir: "./e2e",
  testIgnore: ["screenshots.spec.ts", "demo.spec.ts"],
  retries: 0,
  workers: 1,
  use: {
    baseURL: ORIGIN,
    ignoreHTTPSErrors: TLS,
    trace: "retain-on-failure",
  },
  projects: [{ name: "chromium", use: devices["Desktop Chrome"] }],
  webServer: {
    command:
      `pnpm --dir web build && rm -rf web/${SCRATCH} ` +
      `&& cargo xtask broadcast-fixtures --out web/${SCRATCH}/recordings ` +
      `&& cargo run -q -p sdrmm --no-default-features -- --bind 127.0.0.1:${PORT} ` +
      `--db web/${SCRATCH}/e2e.db --recordings-dir web/${SCRATCH}/recordings` +
      (TLS ? " --tls-self-signed" : ""),
    cwd: "..",
    env: { VITE_ENABLE_SYNTHETIC_DEVICES: "true" },
    url: `${ORIGIN}/api/state`,
    ignoreHTTPSErrors: TLS,
    reuseExistingServer: false,
    timeout: 300_000,
  },
});
