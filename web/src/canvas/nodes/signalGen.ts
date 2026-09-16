export function siggenKey(nodeId: string): string {
  return nodeId.replaceAll(":", "-");
}

export function siggenDeviceId(nodeId: string): string {
  return `siggen:${siggenKey(nodeId)}`;
}
