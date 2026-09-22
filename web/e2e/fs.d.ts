declare module "node:fs/promises" {
  export function mkdir(path: string, options: { recursive: true }): Promise<unknown>;
  export function writeFile(path: string, data: string): Promise<void>;
}
