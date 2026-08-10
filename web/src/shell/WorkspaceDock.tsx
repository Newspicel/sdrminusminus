// One tab's dock (PLAN §10, M6). Compiles the stored layout into dockview, maps every user
// gesture back, and hands the result up to be persisted.
//
// The loop this file exists to break: drag → save → `StateChanged` → refetch → apply → dockview
// emits a layout change → save… Three things stop it. Writes are suppressed while a layout is
// being applied; a layout that maps back to exactly what is stored is not written; and an
// incoming tab equal to the one just emitted is not re-applied.
import {
  type DockviewApi,
  DockviewReact,
  type DockviewReadyEvent,
  type DockviewTheme,
} from "dockview-react";
import { useCallback, useEffect, useRef } from "react";
import type { TabSpec } from "../lib/types";
import { fromSerializedDockview, sameTab, toSerializedDockview } from "./dockLayout";
import { PANEL_COMPONENTS } from "./panels";

/** Quiet time before a layout is written. A drag emits continuously; one write per frame would
 * fan a `StateChanged` to every client at the frame rate. */
const SAVE_DEBOUNCE_MS = 400;

/** The instrumentation theme (PLAN §10). dockview reads `className` for its `--dv-*` variables,
 * which `index.css` defines — no fork of its stylesheet. */
const THEME: DockviewTheme = {
  name: "sdrmm",
  className: "dv-theme-sdrmm",
  colorScheme: "dark",
  gap: 1,
  dndPanelOverlay: "group",
  dndTabIndicator: "line",
  tabGroupIndicator: "none",
};

export function WorkspaceDock({
  tab,
  onChange,
  readOnly,
}: {
  tab: TabSpec;
  /** Called with the mapped-back tab after a user gesture. Never called in `readOnly`. */
  onChange: (tab: TabSpec) => void;
  /** Narrow viewports lay every panel out as one stack and never persist: dockview clamps
   * panels to their minimum size there, and writing the clamp back would flatten the layout a
   * desktop client authored. */
  readOnly: boolean;
}) {
  const apiRef = useRef<DockviewApi | null>(null);
  const hostRef = useRef<HTMLDivElement | null>(null);
  const applyingRef = useRef(false);
  const saveTimer = useRef<number | null>(null);
  // What the dock currently holds, as a stored tab. Compared against both what the dock emits
  // (do not write a no-op) and what arrives from the server (do not re-apply our own echo).
  const currentRef = useRef<TabSpec | null>(null);
  // Which mode the dock currently holds. Crossing the narrow breakpoint changes what is applied
  // without changing the tab, so the stored layout alone cannot tell whether a re-apply is due.
  const modeRef = useRef<boolean | null>(null);
  const tabRef = useRef(tab);
  tabRef.current = tab;
  const onChangeRef = useRef(onChange);
  onChangeRef.current = onChange;
  // The layout listener is registered once, when the dock is ready, so it must not close over
  // the mode: a dock that first mounted narrow would keep suppressing writes after the viewport
  // widened, and every rearrangement would be lost on reload.
  const readOnlyRef = useRef(readOnly);
  readOnlyRef.current = readOnly;

  // The mapped-but-not-yet-written layout. The mapping happens when the gesture lands, not when
  // the debounce fires, so a dock that is unmounted in between (a tab or workspace switch) can
  // still hand the arrangement up: by unmount time React has already disposed the dock, and its
  // `toJSON` no longer describes anything.
  const pendingRef = useRef<TabSpec | null>(null);

  const flush = useCallback(() => {
    if (saveTimer.current !== null) {
      window.clearTimeout(saveTimer.current);
      saveTimer.current = null;
    }
    const pending = pendingRef.current;
    pendingRef.current = null;
    // The mode is re-checked here, not only where the gesture was recorded: crossing the narrow
    // breakpoint mid-debounce would otherwise persist the phone-flattened layout.
    if (pending === null || readOnlyRef.current) {
      return;
    }
    currentRef.current = pending;
    onChangeRef.current(pending);
  }, []);

  const apply = useCallback((next: TabSpec, narrow: boolean) => {
    const api = apiRef.current;
    if (api === null) {
      return;
    }
    const host = hostRef.current;
    const size = {
      width: host?.clientWidth || 1200,
      height: host?.clientHeight || 800,
    };
    applyingRef.current = true;
    // A write still pending belongs to the layout being replaced; letting it land would put the
    // old arrangement back over the new one.
    if (saveTimer.current !== null) {
      window.clearTimeout(saveTimer.current);
      saveTimer.current = null;
    }
    pendingRef.current = null;
    try {
      api.fromJSON(toSerializedDockview(narrow ? flatten(next) : next, size));
      currentRef.current = next;
      modeRef.current = narrow;
    } catch (error) {
      // `fromJSON` clears the dock before rethrowing, so a layout dockview refuses leaves an
      // empty grid rather than a half-applied one. Surface it and stop — silently swallowing
      // would leave the user staring at nothing with no reason given.
      console.error("layout rejected by the dock", error);
    } finally {
      // `onDidLayoutChange` is coalesced into a microtask, so the flag has to outlive this
      // turn; a macrotask is the first point at which the restore's own event has landed.
      window.setTimeout(() => {
        applyingRef.current = false;
      }, 0);
    }
  }, []);

  const onReady = useCallback(
    (event: DockviewReadyEvent) => {
      apiRef.current = event.api;
      apply(tabRef.current, readOnlyRef.current);
      event.api.onDidLayoutChange(() => {
        if (applyingRef.current || readOnlyRef.current) {
          return;
        }
        const mapped = fromSerializedDockview(event.api.toJSON(), tabRef.current);
        if (currentRef.current !== null && sameTab(mapped, currentRef.current)) {
          return;
        }
        pendingRef.current = mapped;
        if (saveTimer.current !== null) {
          window.clearTimeout(saveTimer.current);
        }
        saveTimer.current = window.setTimeout(flush, SAVE_DEBOUNCE_MS);
      });
    },
    [apply, flush],
  );

  // Re-apply when the tab identity changes (a different tab was selected) or when the stored
  // layout differs from what this dock holds — which is how another client's rearrangement
  // arrives. An echo of our own write compares equal and is ignored.
  useEffect(() => {
    const current = currentRef.current;
    if (
      current !== null &&
      modeRef.current === readOnly &&
      current.id === tab.id &&
      sameTab(current, tab)
    ) {
      return;
    }
    apply(tab, readOnly);
  }, [tab, readOnly, apply]);

  // A tab or workspace switch unmounts this dock, and a rearrangement made in the last 400 ms
  // would go with it. The pending write is flushed, not dropped.
  useEffect(() => flush, [flush]);

  return (
    <div ref={hostRef} className="min-h-0 flex-1">
      <DockviewReact
        components={PANEL_COMPONENTS}
        onReady={onReady}
        theme={THEME}
        // Panels keep their DOM when hidden: the waterfall's GL context and the map's camera
        // would otherwise be torn down on every tab switch, and a detached element measures 0×0.
        defaultRenderer="always"
        disableFloatingGroups={readOnly}
        singleTabMode={readOnly ? "fullwidth" : "default"}
      />
    </div>
  );
}

/** Every panel of a tab as one stack. A layout authored on a desktop is unusable at 400 px, and
 * dockview's minimum sizes would rewrite it into something the desktop then inherits. */
function flatten(tab: TabSpec): TabSpec {
  const panels = [];
  const stack = [tab.layout];
  while (stack.length > 0) {
    const node = stack.pop();
    if (node === undefined) {
      break;
    }
    if (node.node === "split") {
      for (let i = node.data.children.length - 1; i >= 0; i--) {
        const child = node.data.children[i];
        if (child !== undefined) {
          stack.push(child.node);
        }
      }
    } else {
      panels.push(...node.data.panels);
    }
  }
  for (const group of tab.floating ?? []) {
    panels.push(...group.group.panels);
  }
  return { id: tab.id, name: tab.name, layout: { node: "group", data: { panels } } };
}
