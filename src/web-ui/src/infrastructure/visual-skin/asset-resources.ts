import type { StoredVisualSkin } from './types';
import { decodeSkinDataUrl } from './archive';
import { referencedSkinAssets } from './part-paint';

/** Display-only media URLs. Persisted packages always retain their original data. */
export class VisualSkinImageResources {
  private readonly generations = new Map<StoredVisualSkin, Map<string, string>>();

  ensure(skin: StoredVisualSkin): ReadonlyMap<string, string> {
    const existing = this.generations.get(skin);
    if (existing) return existing;
    const urls = new Map<string, string>();
    try {
      for (const id of referencedSkinAssets(skin)) {
        const asset = skin.assets[id];
        if (!asset?.dataUrl) continue;
        const bytes = decodeSkinDataUrl(asset.dataUrl);
        const buffer = bytes.buffer instanceof ArrayBuffer
          ? bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength)
          : new Uint8Array(bytes).buffer;
        urls.set(id, URL.createObjectURL(new Blob([buffer], { type: asset.mimeType })));
      }
    } catch (error) {
      urls.forEach(url => URL.revokeObjectURL(url));
      throw error;
    }
    this.generations.set(skin, urls);
    return urls;
  }

  retain(skins: readonly (StoredVisualSkin | null)[]): void {
    for (const [skin, urls] of this.generations) {
      if (skins.includes(skin)) continue;
      urls.forEach(url => URL.revokeObjectURL(url));
      this.generations.delete(skin);
    }
  }
}
