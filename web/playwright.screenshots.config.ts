import { defineConfig, devices } from "@playwright/test";

const PORT = 8098;
const SCRATCH = ".shots-tmp";
const SERVERS = 4;

const FIRST = `http://127.0.0.1:${PORT}`;

export function serverURL(first: string, index: number): string {
  const url = new URL(first);
  url.port = String(Number(url.port) + index);
  return url.origin;
}

function server(index: number, playbackSpeed: number, profile: string) {
  const scratch = `web/${SCRATCH}/${index}`;
  const build =
    index === 0
      ? "pnpm --dir web build"
      : `until curl -sf ${FIRST}/api/state >/dev/null; do sleep 1; done`;
  return {
    command:
      `${build} && rm -rf ${scratch} && mkdir -p ${scratch}/recordings ` +
      `&& cp fixtures/*.sigmf-meta fixtures/*.sigmf-data ${scratch}/recordings/ ` +
      `&& cargo run -q ${profile} -p sdrmm -- --bind 127.0.0.1:${PORT + index} ` +
      `--db ${scratch}/shots.db --recordings-dir ${scratch}/recordings ` +
      `--playback-speed ${playbackSpeed}`,
    cwd: "..",
    env: { VITE_ENABLE_SYNTHETIC_DEVICES: "true" },
    url: `${serverURL(FIRST, index)}/api/state`,
    reuseExistingServer: false,
    timeout: 300_000,
  };
}

export function recordingsConfig(
  testMatch: string,
  playbackSpeed: number,
  profile: string,
  servers: number,
) {
  return defineConfig({
    testDir: "./e2e",
    testMatch,
    retries: 0,
    workers: servers,
    fullyParallel: true,
    timeout: 420_000,
    use: {
      ...devices["Desktop Chrome"],
      baseURL: serverURL(FIRST, 0),
      viewport: { width: 1440, height: 900 },
      deviceScaleFactor: 1.5,
      colorScheme: "dark",
      trace: "retain-on-failure",
      launchOptions: { args: ["--autoplay-policy=no-user-gesture-required"] },
    },
    projects: [{ name: "chromium" }],
    webServer: Array.from({ length: servers }, (_, index) => server(index, playbackSpeed, profile)),
  });
}

export default recordingsConfig("screenshots.spec.ts", 4, "", SERVERS);
