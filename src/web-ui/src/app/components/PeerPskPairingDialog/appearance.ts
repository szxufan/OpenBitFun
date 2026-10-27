import type { AppearanceSurfaceDescriptor } from '@/infrastructure/appearance';

export const peerPskPairingDialogAppearanceDescriptor: AppearanceSurfaceDescriptor = {
  id: 'peer-psk-pairing-dialog',
  parts: [
    { id: 'offer' },
    { id: 'offerCode' },
    { id: 'complete' },
    { id: 'completeCode' },
    { id: 'list' },
    { id: 'entry' },
  ],
};
