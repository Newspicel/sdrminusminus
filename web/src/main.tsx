import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { initTheme } from "./lib/theme";
import "./index.css";

// Before the first render, so the first paint is already in the resolved theme.
initTheme();

// Server state never goes stale on its own: WS `StateChanged` events drive every refetch
// (PLAN §10). No window-focus refetch, no polling.
const queryClient = new QueryClient({
  defaultOptions: {
    queries: { staleTime: Number.POSITIVE_INFINITY, refetchOnWindowFocus: false, retry: 1 },
  },
});

const rootEl = document.getElementById("root");
if (!rootEl) {
  throw new Error("missing #root element");
}
createRoot(rootEl).render(
  <StrictMode>
    <QueryClientProvider client={queryClient}>
      <App />
    </QueryClientProvider>
  </StrictMode>,
);
