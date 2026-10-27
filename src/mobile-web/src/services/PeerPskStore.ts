/**
 * Browser-local pairing secrets for peer devices, one record per
 * (relay, peer device). Mirrors the Rust `remote_connect/psk_store` semantics:
 * `offered` keeps the legacy derivation for traffic, `paired` switches the
 * pairwise key to the PSK derivation. The secret never leaves IndexedDB except
 * into the local key derivation.
 */

import { toB64, fromB64, pskKeyIdOf } from './E2EEncryption';

const DB_NAME = 'openbitfun-mobile-peer-psk';
const DB_VERSION = 1;
const STORE = 'pairings';

export type PeerPskStatus = 'offered' | 'paired';

interface PeerPskRecord {
  /** `${relayUrl}|${peerDeviceId}` */
  id: string;
  relayUrl: string;
  peerDeviceId: string;
  pskB64: string;
  pskId: string;
  status: PeerPskStatus;
  createdAtUnix: number;
}

export interface PeerPskEntry {
  peerDeviceId: string;
  psk: Uint8Array;
  pskId: string;
  status: PeerPskStatus;
  createdAtUnix: number;
}

export interface PeerPskListing {
  peerDeviceId: string;
  pskId: string;
  status: PeerPskStatus;
  createdAtUnix: number;
}

const listeners = new Set<() => void>();
const cache = new Map<string, PeerPskEntry>();
const loaded = new WeakMap<object, Promise<void>>();

function recordId(relayUrl: string, peerDeviceId: string): string {
  return `${relayUrl}|${peerDeviceId}`;
}

function openDatabase(): Promise<IDBDatabase> {
  return new Promise((resolve, reject) => {
    const request = indexedDB.open(DB_NAME, DB_VERSION);
    request.onupgradeneeded = () => {
      if (!request.result.objectStoreNames.contains(STORE)) {
        request.result.createObjectStore(STORE, { keyPath: 'id' });
      }
    };
    request.onerror = () => reject(request.error ?? new Error('pairing store unavailable'));
    request.onsuccess = () => resolve(request.result);
  });
}

async function withStore<T>(
  mode: IDBTransactionMode,
  run: (store: IDBObjectStore) => IDBRequest<T>,
): Promise<T> {
  const db = await openDatabase();
  try {
    return await new Promise<T>((resolve, reject) => {
      const tx = db.transaction(STORE, mode);
      const request = run(tx.objectStore(STORE));
      request.onsuccess = () => resolve(request.result);
      request.onerror = () => reject(request.error ?? new Error('pairing store unavailable'));
      tx.onabort = () => reject(tx.error ?? new Error('pairing store unavailable'));
    });
  } finally {
    db.close();
  }
}

function toEntry(record: PeerPskRecord): PeerPskEntry {
  return {
    peerDeviceId: record.peerDeviceId,
    psk: fromB64(record.pskB64),
    pskId: record.pskId,
    status: record.status,
    createdAtUnix: record.createdAtUnix,
  };
}

function notify(): void {
  for (const listener of listeners) listener();
}

export const PeerPskStore = {
  /** One-time cache warm-up per owner (page) so lookups stay synchronous. */
  async load(scope: object): Promise<void> {
    if (loaded.has(scope)) return loaded.get(scope)!.then(() => undefined);
    const task = (async () => {
      const records = await withStore<PeerPskRecord[]>('readonly', store => store.getAll() as IDBRequest<PeerPskRecord[]>);
      cache.clear();
      for (const record of records) cache.set(record.id, toEntry(record));
    })().catch(error => {
      console.warn('[PeerPskStore] listing unavailable', error);
    });
    loaded.set(scope, task);
    await task;
  },

  onChange(listener: () => void): () => void {
    listeners.add(listener);
    return () => { listeners.delete(listener); };
  },

  lookup(relayUrl: string, peerDeviceId: string): PeerPskEntry | null {
    return cache.get(recordId(relayUrl, peerDeviceId)) ?? null;
  },

  list(relayUrl: string): PeerPskListing[] {
    const prefix = `${relayUrl}|`;
    return Array.from(cache.entries())
      .filter(([id]) => id.startsWith(prefix))
      .map(([, entry]) => ({
        peerDeviceId: entry.peerDeviceId,
        pskId: entry.pskId,
        status: entry.status,
        createdAtUnix: entry.createdAtUnix,
      }))
      .sort((a, b) => a.peerDeviceId.localeCompare(b.peerDeviceId));
  },

  async store(
    relayUrl: string,
    peerDeviceId: string,
    psk: Uint8Array,
    status: PeerPskStatus,
  ): Promise<PeerPskEntry> {
    const entry: PeerPskEntry = {
      peerDeviceId,
      psk,
      pskId: pskKeyIdOf(psk),
      status,
      createdAtUnix: Math.floor(Date.now() / 1000),
    };
    const record: PeerPskRecord = {
      id: recordId(relayUrl, peerDeviceId),
      relayUrl,
      peerDeviceId,
      pskB64: toB64(psk),
      pskId: entry.pskId,
      status,
      createdAtUnix: entry.createdAtUnix,
    };
    await withStore('readwrite', store => store.put(record));
    cache.set(record.id, entry);
    notify();
    return entry;
  },

  /** Promote a stored offer (or an already paired entry) whose id matches. */
  async markPaired(relayUrl: string, peerDeviceId: string, pskId: string): Promise<boolean> {
    const entry = this.lookup(relayUrl, peerDeviceId);
    if (!entry || entry.pskId !== pskId) return false;
    if (entry.status === 'paired') return true;
    await this.store(relayUrl, peerDeviceId, entry.psk, 'paired');
    return true;
  },

  async remove(relayUrl: string, peerDeviceId: string): Promise<boolean> {
    const existed = cache.delete(recordId(relayUrl, peerDeviceId));
    await withStore('readwrite', store => store.delete(recordId(relayUrl, peerDeviceId)));
    if (existed) notify();
    return existed;
  },
};
