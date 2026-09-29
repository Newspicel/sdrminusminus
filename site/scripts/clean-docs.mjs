import { readdir, readFile, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { cleanLinks } from "../src/links.ts";

const root = process.argv[2];
if (root === undefined) {
  console.error("usage: clean-docs.mjs <book directory>");
  process.exit(1);
}

for (const entry of await readdir(root, { recursive: true, withFileTypes: true })) {
  if (entry.isFile() && /\.(html|js)$/.test(entry.name)) {
    const path = join(entry.parentPath, entry.name);
    const text = await readFile(path, "utf8");
    const cleaned = cleanLinks(text);
    if (cleaned !== text) {
      await writeFile(path, cleaned);
    }
  }
}
