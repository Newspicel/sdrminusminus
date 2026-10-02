import { chromium } from "@playwright/test";

const [endpoint, wanted] = process.argv.slice(2);
const receivers = Number(wanted);
const deadline = Date.now() + 60_000;

const browser = await chromium.connectOverCDP(endpoint);
const page = await firstPage(browser);
const speaker = page.locator('.react-flow__node[data-id="speaker"]');
const play = speaker.getByRole("button", { name: "Play", exact: true });
const stop = speaker.getByRole("button", { name: "Stop", exact: true });
const resume = speaker.getByRole("button", { name: "Resume audio" });

await until(async () => (await play.count()) === receivers, "every input on the speaker");
for (const button of await play.elementHandles()) {
  await button.dispatchEvent("click");
}
await until(
  async () => (await stop.count()) === receivers && (await resume.count()) === 0,
  "audio on every input",
);
process.exit(0);

async function firstPage(connected) {
  for (;;) {
    const page = connected.contexts().flatMap((context) => context.pages())[0];
    if (page !== undefined) return page;
    await pause("a page");
  }
}

async function until(ready, what) {
  while (!(await ready())) {
    await pause(what);
  }
}

async function pause(what) {
  if (Date.now() > deadline) {
    console.error(`timed out waiting for ${what}`);
    process.exit(1);
  }
  await new Promise((resolve) => setTimeout(resolve, 250));
}
