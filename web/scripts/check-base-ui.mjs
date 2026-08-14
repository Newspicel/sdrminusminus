import { readdir, readFile } from "node:fs/promises";
import { join, relative } from "node:path";
import { findNativeControls } from "./base-ui-guard.mjs";

const sourceRoot = new URL("../src/", import.meta.url);
const violations = [];

async function inspect(directory) {
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) {
      await inspect(path);
    } else if (entry.name.endsWith(".tsx")) {
      const controls = findNativeControls(await readFile(path, "utf8"));
      if (controls.length > 0) {
        violations.push(`${relative(sourceRoot.pathname, path)}: ${controls.join(", ")}`);
      }
    }
  }
}

await inspect(sourceRoot.pathname);

if (violations.length > 0) {
  console.error("Use @base-ui/react primitives instead of native interactive controls:");
  for (const violation of violations) {
    console.error(`  ${violation}`);
  }
  process.exitCode = 1;
}
