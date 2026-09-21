import { chromium, webkit } from "@playwright/test";
import { createServer } from "vite";

const server = await createServer({
  configFile: false,
  root: process.cwd(),
  server: { host: "127.0.0.1", port: 0, hmr: false, watch: null },
});
await server.listen();
const url = server.resolvedUrls.local[0];
const modes = process.argv.includes("--webkit") ? ["webkit"] : ["metal", "software"];
try {
  for (const mode of modes) {
    const browser = await (mode === "webkit" ? webkit : chromium).launch({
      headless: true,
      ...(mode === "webkit" ? {} : { channel: "chrome" }),
      args: mode === "software" ? ["--use-angle=swiftshader", "--enable-unsafe-swiftshader"] : [],
    });
    try {
      const cases = process.argv.includes("--map-only")
        ? []
        : [
            [1, 640, 240, 4096],
            [4, 480, 180, 4096],
            [8, 300, 120, 1024],
            [1, 1280, 720, 4096],
          ];
      for (const [count, width, height, bins] of cases) {
        for (const renderer of ["webgl", "canvas"]) {
          const page = await browser.newPage({
            viewport: { width: 1800, height: 1200 },
            deviceScaleFactor: 2,
          });
          await page.goto(`${url}benchmarks/gpu.html`);
          const result = await page.evaluate(
            async (options) => {
              const { waterfall } = await import("/benchmarks/gpu.ts");
              return waterfall(options);
            },
            { count, width, height, bins, renderer },
          );
          console.log(
            JSON.stringify({
              mode,
              browser: browser.version(),
              count,
              width,
              height,
              bins,
              renderer,
              ...result,
            }),
          );
          await page.close();
        }
      }
      const page = await browser.newPage({
        viewport: { width: 1280, height: 720 },
        deviceScaleFactor: 2,
      });
      await page.goto(`${url}benchmarks/gpu.html`);
      console.log(
        JSON.stringify({
          mode,
          browser: browser.version(),
          map: await page.evaluate(async () => (await import("/benchmarks/map.ts")).benchmarkMap()),
        }),
      );
      await page.close();
    } finally {
      await browser.close();
    }
  }
} finally {
  await server.close();
}
