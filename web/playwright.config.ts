import { execFileSync } from "node:child_process";
import { defineConfig, devices } from "@playwright/test";

declare const process: { env: Record<string, string | undefined>; execPath: string };

const FREE_PAIR = `
const net = require("node:net");
const bind = (port) =>
  new Promise((resolve, reject) => {
    const server = net.createServer();
    server.once("error", reject);
    server.listen(port, "0.0.0.0", () => {
      const bound = server.address().port;
      server.close(() => resolve(bound));
    });
  });
(async () => {
  for (;;) {
    const port = await bind(0);
    if (port < 65535 && (await bind(port + 1).then(() => true, () => false))) {
      console.log(port);
      return;
    }
  }
})();
`;

function freePort(): string {
  return execFileSync(process.execPath, ["-e", FREE_PAIR], { encoding: "utf8" }).trim();
}

process.env.E2E_PORT ??= freePort();
const PORT = Number(process.env.E2E_PORT);
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
