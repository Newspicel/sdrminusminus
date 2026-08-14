import type { ReactNode } from "react";
import { Empty, EmptyDescription, EmptyHeader } from "@/components/ui/empty";

export function EmptyState({
  children,
  className = "min-h-20 p-4",
}: {
  children: ReactNode;
  className?: string;
}) {
  return (
    <Empty className={className}>
      <EmptyHeader>
        <EmptyDescription>{children}</EmptyDescription>
      </EmptyHeader>
    </Empty>
  );
}
