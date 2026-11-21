/** Live text items and restored text items can have different IDs for the same reply. */
export function readWidgetState(storage: Storage, key: string, legacyKey: string, roundId: string) {
  const saved = storage.getItem(key);
  if (saved !== null) return saved;
  let previous = storage.getItem(legacyKey);
  if (previous === null) {
    const scope = JSON.parse(legacyKey) as string[];
    const prefix = JSON.stringify(scope.slice(0, 3)).slice(0, -1) + ',';
    for (let i = 0; i < storage.length; i++) {
      const candidate = storage.key(i);
      if (!candidate?.startsWith(prefix)) continue;
      try {
        const parts = JSON.parse(candidate);
        const liveId = parts[3];
        if (
          typeof liveId !== 'string' ||
          !liveId.startsWith(roundId + '-text-') ||
          !/^\d+$/.test(liveId.slice(roundId.length + 6))
        )
          continue;
        scope[3] = liveId;
        if (JSON.stringify(scope) !== candidate) continue;
        previous = storage.getItem(candidate);
        break;
      } catch {
        /* A malformed key is not a previous reader state. */
      }
    }
  }
  if (previous !== null) {
    try {
      storage.setItem(key, previous);
    } catch {
      /* Read the previous copy when storage is full. */
    }
  }
  return previous;
}
