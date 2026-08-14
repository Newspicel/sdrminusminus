import { Toaster } from "@/components/ui/sonner";

/** Long enough to notice and read a server message, short enough that a stale one is not still
 * on screen when the next thing goes wrong. */
export function Toasts() {
  return <Toaster position="bottom-right" duration={12_000} visibleToasts={4} closeButton />;
}
