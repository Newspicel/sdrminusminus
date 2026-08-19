import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { FieldApp } from "./field/FieldApp";
import { isFieldPath } from "./field/missions";
import { adoptTokenFromUrl } from "./lib/auth";
import { initTheme } from "./lib/theme";
import "./index.css";

initTheme();
adoptTokenFromUrl(window.location, window.history);

const queryClient = new QueryClient({
  defaultOptions: {
    queries: { staleTime: Number.POSITIVE_INFINITY, refetchOnWindowFocus: false, retry: 1 },
  },
});

const rootEl = document.getElementById("root");
if (!rootEl) {
  throw new Error("missing #root element");
}
const Root = isFieldPath(window.location.pathname) ? FieldApp : App;

createRoot(rootEl).render(
  <StrictMode>
    <QueryClientProvider client={queryClient}>
      <Root />
    </QueryClientProvider>
  </StrictMode>,
);
