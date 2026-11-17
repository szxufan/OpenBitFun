import type { FileEntry, ZipReader } from '@zip.js/zip.js';

/** The only size limit in the skin pipeline: one file, 128 MiB. */
export const MAX_SKIN_ASSET_BYTES = 128 * 1024 * 1024;

/** Metadata preflight and a bounded output sink; no unbounded async() reads. */
export class SkinArchive {
  readonly files = new Map<string, FileEntry>();
  private constructor(private readonly reader: ZipReader<Uint8Array>) {}

  static async open(source: ArrayBuffer): Promise<SkinArchive> {
    // Skin import is a user action; keep the archive reader out of the startup bundle.
    const zip = await import('@zip.js/zip.js');
    const reader = new zip.ZipReader(new zip.Uint8ArrayReader(new Uint8Array(source)), {
      useWebWorkers: false, checkSignature: true, checkOverlappingEntry: true,
    });
    const archive = new SkinArchive(reader);
    try {
      for await (const entry of reader.getEntriesGenerator()) {
        const path = entry.directory ? entry.filename.replace(/\/$/, '') : entry.filename;
        if (!path || path.startsWith('/') || path.includes('\\') || path.split('/').some(segment => !segment || segment === '.' || segment === '..')) {
          throw new Error(`Visual skin archive path is unsafe: ${entry.filename}`);
        }
        if (!Number.isSafeInteger(entry.uncompressedSize) || entry.uncompressedSize < 0 || entry.uncompressedSize > MAX_SKIN_ASSET_BYTES) {
          throw new Error(`Visual skin archive entry exceeds its size limit: ${entry.filename}`);
        }
        if (entry.directory) continue;
        if (entry.encrypted) throw new Error('Encrypted visual skin archives are unsupported');
        if (archive.files.has(entry.filename)) throw new Error(`Duplicate visual skin archive path: ${entry.filename}`);
        archive.files.set(entry.filename, entry);
      }
      if (!archive.files.size) throw new Error('Visual skin archive is empty');
      return archive;
    } catch (error) { await reader.close(); throw error; }
  }

  async read(path: string, limit: number = MAX_SKIN_ASSET_BYTES): Promise<Uint8Array> {
    const entry = this.files.get(path);
    if (!entry) throw new Error(`Visual skin archive file is missing: ${path}`);
    if (entry.uncompressedSize > limit) throw new Error(`Visual skin archive entry exceeds its size limit: ${path}`);
    let size = 0;
    const chunks: Uint8Array[] = [];
    const abort = new AbortController();
    try {
      await entry.getData(new WritableStream<Uint8Array>({
        write: chunk => {
          size += chunk.byteLength;
          // A forged directory can understate its size; the per-file budget is
          // therefore enforced against the real stream as well.
          if (size > limit) {
            const error = new Error(`Visual skin decompression size limit exceeded: ${path}`);
            abort.abort(error);
            throw error;
          }
          chunks.push(chunk);
        },
      }), { signal: abort.signal, checkSignature: true, useWebWorkers: false });
      if (size !== entry.uncompressedSize) throw new Error(`Visual skin archive size mismatch: ${path}`);
      const bytes = new Uint8Array(size);
      let offset = 0;
      for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.length; }
      return bytes;
    } catch (error) { abort.abort(error); throw error; }
  }

  async close(): Promise<void> { await this.reader.close(); }
}

type Base64Uint8Array = Uint8Array & { toBase64?: () => string };
type Base64Uint8ArrayConstructor = typeof Uint8Array & { fromBase64?: (value: string) => Uint8Array };

export function skinAssetDataUrl(mime: string, bytes: Uint8Array): string {
  const native = (bytes as Base64Uint8Array).toBase64;
  if (typeof native === 'function') {
    return `data:${mime};base64,${native.call(bytes)}`;
  }
  // 32766 is a multiple of 3, so each chunk encodes without interior '=' padding.
  const parts = [`data:${mime};base64,`];
  for (let offset = 0; offset < bytes.length; offset += 32766) {
    parts.push(btoa(String.fromCharCode(...bytes.subarray(offset, offset + 32766))));
  }
  return parts.join('');
}

export function decodeSkinDataUrl(dataUrl: string): Uint8Array {
  const payload = dataUrl.slice(dataUrl.indexOf(',') + 1);
  const fromBase64 = (Uint8Array as Base64Uint8ArrayConstructor).fromBase64;
  if (typeof fromBase64 === 'function') {
    return fromBase64(payload);
  }
  const binary = atob(payload);
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index++) bytes[index] = binary.charCodeAt(index);
  return bytes;
}
