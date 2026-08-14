import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { TooltipProvider } from "./components/ui/tooltip";
import { initTheme } from "./lib/theme";
import "./styles.css";

// Before the first render, so the first paint is already in the resolved theme.
initTheme();

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
      <TooltipProvider>
        <App />
      </TooltipProvider>
    </QueryClientProvider>
  </StrictMode>,
);
