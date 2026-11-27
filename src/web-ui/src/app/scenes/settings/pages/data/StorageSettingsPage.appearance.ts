import type { AppearanceSurfaceDescriptor } from '@/infrastructure/appearance/types';

export const snapshotStorageAppearanceDescriptor: AppearanceSurfaceDescriptor = {
  id: 'snapshot-storage',
  parts: [
    { id: 'root' }, { id: 'panel' }, { id: 'total' }, { id: 'workspaceList' },
    { id: 'filters' }, { id: 'candidateList' }, { id: 'session' }, { id: 'reviewFooter' },
  ],
};
