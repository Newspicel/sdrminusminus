const NATIVE_CONTROL =
  /<(button|datalist|details|dialog|form|input|meter|option|progress|select|summary|textarea)\b/g;

export function findNativeControls(source) {
  return [...source.matchAll(NATIVE_CONTROL)].map((match) => match[1]);
}
