import type {
  HuntSweepParams,
  NodeBody,
  NodeBodyOf,
  NodeKind,
  PatchCatalog,
  PatchNodeOf,
  ProcessorKind,
} from "../lib/types";

export interface NewNodeSeed {
  channelType?: string;
}

export type SettingsKind = ProcessorKind;

type SettingsMap = {
  [K in SettingsKind]: NonNullable<NodeBodyOf<K>["data"]["settings"]>;
};

export type SettingsOf<K extends SettingsKind> = SettingsMap[K];

type ProcessorNode = PatchNodeOf<SettingsKind>;

export function defaultBody(catalog: PatchCatalog, kind: NodeKind): NodeBody | null {
  const entry = catalog.nodes.find((type) => type.kind === kind);
  return entry === undefined || entry.default_body.kind !== kind
    ? null
    : structuredClone(entry.default_body);
}

export function newNodeBody(
  catalog: PatchCatalog,
  kind: NodeKind,
  seed: NewNodeSeed = {},
): NodeBody | null {
  if (kind === "channel") {
    return {
      kind,
      data: { channel_type: seed.channelType ?? "nfm" },
    };
  }
  return defaultBody(catalog, kind);
}

export function defaultSettings<K extends SettingsKind>(
  catalog: PatchCatalog,
  kind: K,
): SettingsOf<K> | null {
  const body = defaultBody(catalog, kind) as NodeBodyOf<SettingsKind> | null;
  return (body?.data.settings as SettingsOf<K> | undefined) ?? null;
}

export function settingsOf<K extends SettingsKind>(
  node: PatchNodeOf<K>,
  catalog: PatchCatalog,
): SettingsOf<K> | null {
  const typed = node as unknown as ProcessorNode;
  const stored = typed.data.settings as SettingsOf<K> | undefined;
  return stored ?? defaultSettings(catalog, typed.kind as K);
}

export function carriesSettings(catalog: PatchCatalog, kind: NodeKind): boolean {
  const body = defaultBody(catalog, kind);
  return body !== null && "data" in body;
}

export function startsOnItsOwn(kind: NodeKind): boolean {
  return kind === "signal_gen";
}

export function huntSweepOf(
  node: PatchNodeOf<"hunt">,
  catalog: PatchCatalog,
): HuntSweepParams | null {
  if (node.data?.sweep !== undefined) {
    return node.data.sweep;
  }
  const body = defaultBody(catalog, "hunt");
  return body?.kind === "hunt" ? (body.data?.sweep ?? null) : null;
}
