// The smoke flow the web suite owed since M6 (PLAN §14, CANVAS §8). One browser, one server,
// one path through the workspace — enough to catch the class of break vitest cannot see, where
// every pure transform is right and the composition above them never mounts.
//
// The server under test is the real one on `device-virtual` (PLAN §14: no hardware in CI, ever)
// with a throwaway database, so the run starts from the seeded default workspace every time and
// leaves nothing behind.
import { defineConfig, devices } from "@playwright/test";

const PORT = 8099;
/** Throwaway state for the run, inside `web/` and git-ignored: the flow must start from the
 * seeded default workspace every time, and must not touch the developer's own database. */
const SCRATCH = ".e2e-tmp";

export default defineConfig({
  testDir: "./e2e",
  // No retries: the flow mutates server state (it opens a radio and stores an arrangement), so a
  // second attempt would start from what the first one left and would not be the same test. A
  // failure here is a real break to read, not a flake to re-roll.
  retries: 0,
  use: {
    baseURL: `http://127.0.0.1:${PORT}`,
    trace: "retain-on-failure",
  },
  projects: [{ name: "chromium", use: devices["Desktop Chrome"] }],
  webServer: {
    // The server serves the built UI itself, which is also how a release artifact runs — the
    // Vite dev server would test a composition the user never gets.
    command: `rm -rf web/${SCRATCH} && cargo run -q -p sdrmm -- --bind 127.0.0.1:${PORT} --db web/${SCRATCH}/e2e.db --recordings-dir web/${SCRATCH}/recordings`,
    cwd: "..",
    url: `http://127.0.0.1:${PORT}/api/state`,
    reuseExistingServer: false,
    timeout: 180_000,
  },
});
