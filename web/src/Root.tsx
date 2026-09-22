import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { type ReactNode, StrictMode } from "react";
import { ErrorBoundary } from "./components/ErrorBoundary";

export function createQueryClient(): QueryClient {
  return new QueryClient({
    defaultOptions: {
      queries: { staleTime: Number.POSITIVE_INFINITY, refetchOnWindowFocus: false, retry: 1 },
    },
  });
}

export function Root({ client, children }: { client: QueryClient; children: ReactNode }) {
  return (
    <StrictMode>
      <QueryClientProvider client={client}>
        <ErrorBoundary>{children}</ErrorBoundary>
      </QueryClientProvider>
    </StrictMode>
  );
}
