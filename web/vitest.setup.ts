import { vi } from "vitest";
import type { StateCreator, StoreApi } from "zustand/vanilla";

function memoryStorage(): Storage {
  const items = new Map<string, string>();
  return {
    get length() {
      return items.size;
    },
    clear: () => items.clear(),
    getItem: (key: string) => items.get(key) ?? null,
    key: (index: number) => [...items.keys()][index] ?? null,
    removeItem: (key: string) => {
      items.delete(key);
    },
    setItem: (key: string, value: string) => {
      items.set(key, value);
    },
  };
}

Object.defineProperty(globalThis, "localStorage", {
  value: memoryStorage(),
  configurable: true,
  writable: true,
});

vi.mock("zustand", async () => {
  const react = await import("react");
  const vanilla = await import("zustand/vanilla");
  function useLiveStore<T, U>(api: StoreApi<T>, selector: (state: T) => U): U {
    const read = (): U => selector(api.getState());
    return react.useSyncExternalStore(api.subscribe, read, read);
  }
  function bind<T>(createState: StateCreator<T>) {
    const api = vanilla.createStore<T>()(createState);
    const hook = <U = T>(selector?: (state: T) => U): U =>
      useLiveStore(api, selector ?? ((state: T) => state as unknown as U));
    return Object.assign(hook, api);
  }
  function create<T>(createState?: StateCreator<T>) {
    return createState === undefined ? bind : bind(createState);
  }
  return { create, useStore: useLiveStore };
});
