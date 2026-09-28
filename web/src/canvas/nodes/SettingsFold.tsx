import type { ReactNode } from "react";
import { Settings } from "../../components/Settings";
import { FoldSection } from "./FoldSection";

export function SettingsFold({
  label,
  open = false,
  children,
}: {
  label: string;
  open?: boolean;
  children: ReactNode;
}) {
  return (
    <FoldSection label={label} open={open}>
      <Settings>{children}</Settings>
    </FoldSection>
  );
}
