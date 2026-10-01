import { Collapsible } from "@base-ui/react/collapsible";

export function FaceFault({ message, detail }: { message: string; detail?: string | null }) {
  return (
    <div role="alert" className="border-t border-line p-2 text-xs text-danger">
      {detail == null || detail === message ? (
        <p className="wrap-anywhere">{message}</p>
      ) : (
        <Collapsible.Root>
          <Collapsible.Trigger className="cursor-pointer text-left">{message}</Collapsible.Trigger>
          <Collapsible.Panel>
            <p className="mt-1 font-mono wrap-anywhere text-ink-dim">{detail}</p>
          </Collapsible.Panel>
        </Collapsible.Root>
      )}
    </div>
  );
}
