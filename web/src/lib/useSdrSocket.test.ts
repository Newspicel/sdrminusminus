import { QueryClient } from "@tanstack/react-query";
import { describe, expect, it } from "vitest";
import { PHONES_KEY, STATE_KEY } from "./api";
import { invalidateScope } from "./useSdrSocket";

function seeded(): QueryClient {
  const client = new QueryClient();
  client.setQueryData(PHONES_KEY, { phones: [] });
  client.setQueryData(STATE_KEY, { device_sets: [] });
  return client;
}

describe("invalidateScope", () => {
  it("refreshes the phone list when phones change", () => {
    const client = seeded();
    invalidateScope(client, { scope: "phones" });
    expect(client.getQueryState(PHONES_KEY)?.isInvalidated).toBe(true);
    expect(client.getQueryState(STATE_KEY)?.isInvalidated).toBe(false);
  });

  it("leaves every query alone when missions change", () => {
    const client = seeded();
    invalidateScope(client, { scope: "missions" });
    expect(client.getQueryState(PHONES_KEY)?.isInvalidated).toBe(false);
    expect(client.getQueryState(STATE_KEY)?.isInvalidated).toBe(false);
  });
});
