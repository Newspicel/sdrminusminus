import { readdir, readFile } from "node:fs/promises";
import { join, relative } from "node:path";
import {
  findDirectBaseUiImports,
  findUnwrappedControls,
  isInsideDirectory,
} from "./shadcn-guard.mjs";

const sourceRoot = new URL("../src/", import.meta.url);
const uiRoot = join(sourceRoot.pathname, "components", "ui");
const violations = [];

async function inspect(directory) {
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) {
      await inspect(path);
    } else if (entry.name.endsWith(".tsx") && !isInsideDirectory(path, uiRoot)) {
      const source = await readFile(path, "utf8");
      const controls = findUnwrappedControls(source);
      const imports = findDirectBaseUiImports(source);
      if (controls.length > 0 || imports.length > 0) {
        violations.push(
          `${relative(sourceRoot.pathname, path)}: ${[...controls, ...imports].join(", ")}`,
        );
      }
    }
  }
}

await inspect(sourceRoot.pathname);

if (violations.length > 0) {
  console.error(
    "Use components from src/components/ui instead of raw controls or Base UI imports:",
  );
  for (const violation of violations) console.error(`  ${violation}`);
  process.exitCode = 1;
}
