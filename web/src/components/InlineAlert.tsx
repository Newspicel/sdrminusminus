import type { ReactNode } from "react";
import { Alert, AlertDescription } from "@/components/ui/alert";

export function InlineAlert({ children, className }: { children: ReactNode; className?: string }) {
  return (
    <Alert variant="destructive" className={className}>
      <AlertDescription>{children}</AlertDescription>
    </Alert>
  );
}
