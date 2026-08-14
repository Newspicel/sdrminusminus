import { sep } from "node:path";

const NATIVE_CONTROL =
  /<(button|datalist|details|dialog|input|meter|option|progress|select|summary|textarea)\b/g;
const BASE_UI_IMPORT = /from\s+["']@base-ui\/react(?:\/[^"']*)?["']/g;

export function findUnwrappedControls(source) {
  return [...source.matchAll(NATIVE_CONTROL)].map((match) => match[1]);
}

export function findDirectBaseUiImports(source) {
  return [...source.matchAll(BASE_UI_IMPORT)].map((match) => match[0]);
}

export function isInsideDirectory(path, directory) {
  return path.startsWith(`${directory}${sep}`);
}
