import { fetchFigureAsset } from './figureResources';
import type { BoundValues, Declarations } from './visualBindings';

const maxImageBytes = 256 * 1024;
const raster = /^image\/(?:png|jpeg|webp|gif|avif)$/;

export async function localizeFigureImages(declarations: Declarations, values: BoundValues): Promise<BoundValues> {
  const local = { ...values };
  await Promise.all(Object.entries(declarations).map(async ([key, binding]) => {
    if (binding.type !== 'image') return;
    const existing = values[key];
    if (existing?.kind === 'image' && typeof existing.value === 'string' &&
        /^data:image\/(?:png|jpeg|webp|gif|avif);base64,/.test(existing.value)) return;
    try {
      local[key] = { kind: 'image', value: await fetchFigureAsset(binding.url, raster, maxImageBytes) };
    } catch {
      throw Error('Could not load the declared image ' + key + '. Use an embedded image or a CORS-enabled raster image up to 256 KiB.');
    }
  }));
  return local;
}
