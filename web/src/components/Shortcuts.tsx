import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogTitle,
} from "@/components/ui/dialog";
import { Kbd } from "@/components/ui/kbd";
import { BINDINGS } from "../canvas/useHotkeys";

export function Shortcuts({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-md" showCloseButton={false}>
        <div className="flex items-baseline justify-between gap-4">
          <DialogTitle className="text-base font-medium text-foreground">Keyboard</DialogTitle>
          <DialogDescription className="font-mono text-[10px] leading-[1.4] tracking-[0.09em] uppercase text-muted-foreground/70">
            Ignored while a field has focus
          </DialogDescription>
        </div>
        <dl className="mt-3 grid grid-cols-[8rem_1fr] gap-x-4 gap-y-1.5">
          {BINDINGS.map((binding) => (
            <div key={binding.keys} className="contents">
              <dt className="text-right">
                <Kbd>{binding.keys}</Kbd>
              </dt>
              <dd className="text-xs text-muted-foreground">{binding.what}</dd>
            </div>
          ))}
        </dl>
        <DialogFooter className="mt-4">
          <DialogClose render={<Button variant="outline" size="sm" />}>Close</DialogClose>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
