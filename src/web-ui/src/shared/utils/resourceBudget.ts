export interface BudgetedResource {
  /** Logical retained bytes, not process memory or GPU memory. */
  bytes: number;
  kind: 'derived' | 'history';
  lastUsedAt: number;
  /** Re-evaluated at eviction time; a lease must never be a stale render flag. */
  protectedReason?: () => string | undefined;
  evict: () => void;
}

/** Coordinates reconstructable caches. Documents, drafts and runtime state are not caches. */
export class ResourceBudget {
  private resources = new Map<object, BudgetedResource>();
  private timer: ReturnType<typeof setTimeout> | undefined;
  private trimming = false;

  constructor(readonly softBytes = 64 * 1024 * 1024, readonly hardBytes = 96 * 1024 * 1024) {}

  set(key: object, resource: BudgetedResource): void {
    this.resources.set(key, resource);
    this.schedule();
  }

  touch(key: object): void {
    const resource = this.resources.get(key);
    if (resource) resource.lastUsedAt = Date.now();
  }

  delete(key: object): void { this.resources.delete(key); }

  get byteSize(): number {
    let bytes = 0;
    for (const resource of this.resources.values()) bytes += resource.bytes;
    return bytes;
  }

  canPrefetch(): boolean { return this.byteSize < this.softBytes; }

  snapshot() {
    return [...this.resources.values()].map(resource => ({
      kind: resource.kind, bytes: resource.bytes, lastUsedAt: resource.lastUsedAt,
      protectedReason: resource.protectedReason?.(),
    }));
  }

  /** A host adapter may request pressure relief without reaching into product owners. */
  trim(pressure = false): void {
    if (this.trimming) return;
    this.trimming = true;
    try {
      let bytes = this.byteSize;
      if (!pressure && bytes <= this.hardBytes) return;
      const candidates = [...this.resources.entries()].sort(([, a], [, b]) =>
        Number(a.kind === 'history') - Number(b.kind === 'history') || a.lastUsedAt - b.lastUsedAt);
      for (const [key, resource] of candidates) {
        if (bytes <= this.softBytes) break;
        if (resource.protectedReason?.()) continue;
        // Remove accounting before invoking the owner; disposal can synchronously
        // publish state and register a newer resource under the same key.
        this.resources.delete(key);
        bytes -= resource.bytes;
        resource.evict();
      }
    } finally {
      this.trimming = false;
    }
  }

  private schedule(): void {
    if (this.timer !== undefined) return;
    // Coalesce writes outside rendering. Age alone does not justify turning a
    // warm session into a blocking read, and needs no recurring sweep.
    this.timer = setTimeout(() => {
      this.timer = undefined;
      this.trim();
    }, 0);
  }
}

export const resourceBudget = new ResourceBudget();

declare global {
  var __OPENBITFUN_RESOURCE_BUDGET__: Pick<ResourceBudget, 'snapshot'> | undefined;
}
// Read-only logical accounting for native performance diagnostics. No cache
// mutation, authoritative payloads, DOM references or forced collection API.
if (import.meta.env.DEV || globalThis.__OPENBITFUN_PERF_TRACE_ENABLED__ === true) {
  globalThis.__OPENBITFUN_RESOURCE_BUDGET__ = { snapshot: () => resourceBudget.snapshot() };
}

interface CachedSettledRoundBytes {
  bytes: number;
  objects: object[];
}

const settledRoundByteCache = new WeakMap<object, CachedSettledRoundBytes>();

function isSettledModelRound(value: object): boolean {
  const candidate = value as {
    status?: unknown;
    isComplete?: unknown;
    isStreaming?: unknown;
    items?: unknown;
  };
  return (
    candidate.status === 'completed' &&
    candidate.isComplete === true &&
    candidate.isStreaming === false &&
    Array.isArray(candidate.items)
  );
}

function indexKeyLength(index: number): number {
  if (index < 10) return 1;
  if (index < 100) return 2;
  if (index < 1000) return 3;
  if (index < 10000) return 4;
  return String(index).length;
}

function measureSettledModelRound(round: object, outerSeen: Set<object>): number {
  const cached = settledRoundByteCache.get(round);
  if (cached) {
    const { objects } = cached;
    let hasOverlap = false;
    for (let i = 0; i < objects.length; i += 1) {
      if (outerSeen.has(objects[i])) {
        hasOverlap = true;
        break;
      }
    }
    if (!hasOverlap) {
      for (let i = 0; i < objects.length; i += 1) {
        outerSeen.add(objects[i]);
      }
      return cached.bytes;
    }
  }

  const localSeen = new Set<object>();
  let hadOuterOverlap = false;

  const walk = (node: unknown): number => {
    if (typeof node === 'string') return node.length * 2;
    if (!node || typeof node !== 'object') return 8;
    if (localSeen.has(node)) return 0;
    if (outerSeen.has(node)) {
      hadOuterOverlap = true;
      return 0;
    }
    localSeen.add(node);
    outerSeen.add(node);

    let bytes = 32;
    if (Array.isArray(node)) {
      for (let i = 0; i < node.length; i += 1) {
        if (!(i in node)) continue;
        bytes += indexKeyLength(i) * 2 + 8 + walk(node[i]);
      }
      return bytes;
    }
    for (const key in node) {
      if (!Object.prototype.hasOwnProperty.call(node, key)) continue;
      bytes += key.length * 2 + 8 + walk((node as Record<string, unknown>)[key]);
    }
    return bytes;
  };

  const bytes = walk(round);
  if (!cached && !hadOuterOverlap) {
    settledRoundByteCache.set(round, {
      bytes,
      objects: Array.from(localSeen),
    });
  }
  return bytes;
}

/** No stringify-sized temporary copy. Shared references are counted once per working set. */
export function estimateRetainedBytes(value: unknown, seen = new Set<object>()): number {
  if (typeof value === 'string') return value.length * 2;
  if (!value || typeof value !== 'object') return 8;
  if (seen.has(value)) return 0;
  if (!Array.isArray(value) && isSettledModelRound(value)) {
    return measureSettledModelRound(value, seen);
  }
  seen.add(value);
  let bytes = 32;
  if (Array.isArray(value)) {
    for (let i = 0; i < value.length; i += 1) {
      if (!(i in value)) continue;
      bytes += indexKeyLength(i) * 2 + 8 + estimateRetainedBytes(value[i], seen);
    }
    return bytes;
  }
  for (const key in value) {
    if (!Object.prototype.hasOwnProperty.call(value, key)) continue;
    bytes += key.length * 2 + 8 + estimateRetainedBytes((value as Record<string, unknown>)[key], seen);
  }
  return bytes;
}
