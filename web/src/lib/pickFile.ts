export function pickFile(accept: string): Promise<File | null> {
  return new Promise((resolve) => {
    const input = document.createElement("input");
    input.type = "file";
    input.accept = accept;
    input.style.display = "none";
    const settle = (file: File | null) => {
      input.remove();
      resolve(file);
    };
    input.addEventListener("change", () => settle(input.files?.[0] ?? null), { once: true });
    input.addEventListener("cancel", () => settle(null), { once: true });
    document.body.append(input);
    input.click();
  });
}
