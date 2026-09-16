const outputs = new Map<string, () => number>();

export function registerMediaLatency(key: string, latency: () => number): () => void {
  outputs.set(key, latency);
  return () => {
    if (outputs.get(key) === latency) outputs.delete(key);
  };
}

export function mediaLatencyMs(key: string): number {
  const delay = outputs.get(key)?.() ?? 0;
  return Number.isFinite(delay) ? Math.min(500, Math.max(0, delay)) : 0;
}
