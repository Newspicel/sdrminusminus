import { beforeEach, describe, expect, it, vi } from "vitest";
import { cancelLeave, confirmLeave, leaveAuto, useAutoOff } from "./autoOff";

function memoryStorage(): Storage {
  const items = new Map<string, string>();
  return {
    get length() {
      return items.size;
    },
    clear: () => items.clear(),
    getItem: (key) => items.get(key) ?? null,
    key: (index) => [...items.keys()][index] ?? null,
    removeItem: (key) => items.delete(key),
    setItem: (key, value) => items.set(key, value),
  };
}

describe("leaveAuto", () => {
  beforeEach(() => {
    vi.stubGlobal("localStorage", memoryStorage());
    useAutoOff.setState({ pending: null });
  });

  it("tunes at once when the radio is already manual", () => {
    const proceed = vi.fn();
    leaveAuto(false, proceed);
    expect(proceed).toHaveBeenCalledOnce();
  });

  it("asks first on Auto and tunes only once confirmed", () => {
    const proceed = vi.fn();
    leaveAuto(true, proceed);
    expect(proceed).not.toHaveBeenCalled();
    confirmLeave(false);
    expect(proceed).toHaveBeenCalledOnce();
    leaveAuto(true, proceed);
    expect(useAutoOff.getState().pending).not.toBeNull();
  });

  it("drops the change when cancelled", () => {
    const proceed = vi.fn();
    leaveAuto(true, proceed);
    cancelLeave();
    expect(proceed).not.toHaveBeenCalled();
    expect(useAutoOff.getState().pending).toBeNull();
  });

  it("stops asking once told never again", () => {
    leaveAuto(true, vi.fn());
    confirmLeave(true);
    const proceed = vi.fn();
    leaveAuto(true, proceed);
    expect(proceed).toHaveBeenCalledOnce();
  });
});
