const PAGE_LINK = /"(?!\/\/)([\w./-]*?)\.html((?:[#?][^"\s]*)?)"/g;

function page(path: string): string {
  return path.replace(/(^|\/)index$/, "$1") || "./";
}

export function cleanLinks(text: string): string {
  return text.replace(PAGE_LINK, (_, path: string, rest: string) => `"${page(path)}${rest}"`);
}
