const NATIVE_CONTROL =
  /<(button|details|dialog|form|input|meter|progress|select|summary|textarea)\b/g;

export function findNativeControls(source) {
  return [...source.matchAll(NATIVE_CONTROL)].map((match) => match[1]);
}
