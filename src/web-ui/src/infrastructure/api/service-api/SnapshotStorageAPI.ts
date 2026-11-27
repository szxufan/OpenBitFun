import { api } from './ApiClient';

export type StoragePeriod = 'sevenDays' | 'thirtyDays' | 'all';

export interface SnapshotCandidate {
  sessionId: string;
  name: string;
  activeAt: number;
  revision: string;
  removed?: boolean;
}
export interface WorkspaceUsage {
  workspaceId: string;
  name: string;
  location: string;
  bytes: number;
  sharedBytes: number;
  estimatedBytes: number;
  removedEstimatedBytes?: number;
  skippedSessions: number;
  candidates: SnapshotCandidate[];
  warning: string | null;
  unverifiedReferences: boolean;
}
export interface StorageOverview {
  scanErrors: string[];
  totalBytes: number;
  unidentifiedBytes: number;
  workspaces: WorkspaceUsage[];
}
interface CleanupResult {
  removedSessions: number;
  skippedSessions: number;
  freedBytes: number;
  errors: string[];
}

export const snapshotStorageAPI = {
  inspect: (period: StoragePeriod) =>
    api.invoke<StorageOverview>('inspect_snapshot_storage', { request: { period } }, { timeout: 0, retries: 0 }),
  clean: (workspaceId: string, period: StoragePeriod, candidates: SnapshotCandidate[]) =>
    api.invoke<CleanupResult>('clean_snapshot_storage', { request: { workspaceId, period, candidates } }, { timeout: 0, retries: 0 }),
};
