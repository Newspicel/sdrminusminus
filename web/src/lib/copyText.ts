export async function copyText(text: string): Promise<void> {
  if (await writeToClipboard(text)) {
    return;
  }
  if (copyBySelection(text)) {
    return;
  }
  throw new Error("copying needs HTTPS or a localhost address in this browser");
}

async function writeToClipboard(text: string): Promise<boolean> {
  if (navigator.clipboard === undefined) {
    return false;
  }
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    return false;
  }
}

function copyBySelection(text: string): boolean {
  const focused = document.activeElement;
  const field = document.createElement("textarea");
  field.value = text;
  field.readOnly = true;
  field.style.position = "fixed";
  field.style.top = "0";
  field.style.opacity = "0";
  document.body.append(field);
  field.select();
  const copied = document.execCommand("copy");
  field.remove();
  if (focused !== null) {
    (focused as HTMLElement).focus?.();
  }
  return copied;
}
