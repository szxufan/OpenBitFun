import type { Declarations } from './visualBindings';
import release from '../../../../../scripts/visualize-runtime.json';

const loadTimeout = 4000;

export async function fetchFigureAsset(url: string, mime: RegExp, maxBytes: number, integrity?: string, timeout = loadTimeout): Promise<string> {
  // This is a single anonymous browser GET for an immutable declaration.
  // Guest scripts have no bridge for constructing requests or following redirects.
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), timeout);
  try {
    const response = await fetch(url, {
      integrity, credentials: 'omit', referrerPolicy: 'no-referrer', mode: 'cors', redirect: 'error',
      signal: controller.signal,
    });
    const type = response.headers.get('content-type')?.split(';')[0].trim() ?? '';
    if (!response.ok || !mime.test(type) || !response.body ||
        Number(response.headers.get('content-length')) > maxBytes) throw Error('Invalid figure asset');
    const reader = response.body.getReader();
    const chunks: BlobPart[] = [];
    let size = 0;
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      size += value.byteLength;
      if (size > maxBytes) throw Error('Figure asset exceeds the allowed size');
      chunks.push(new Uint8Array(value).buffer);
    }
    if (!size) throw Error('Empty figure asset');
    return await new Promise<string>((resolve, reject) => {
      const file = new FileReader();
      file.onload = () => resolve(String(file.result));
      file.onerror = () => reject(Error('Unreadable figure asset'));
      file.readAsDataURL(new Blob(chunks, { type }));
    });
  } finally {
    controller.abort();
    clearTimeout(timer);
  }
}

const bundledUrls = new Set(Object.values(release).map(asset => asset.url));
const scriptOrigins = new Set(['https://cdnjs.cloudflare.com', 'https://cdn.jsdelivr.net', 'https://esm.sh', 'https://unpkg.com']);
const scriptMime = /^(?:text|application)\/(?:x-)?(?:java|ecma)script$/i;
const maxSourceBytes = 1024 * 1024;

// Freeze fixed classic script tags from the previously supported public CDNs.
// Resolve packaged modules through the normal runtime. The guest has no network bridge.
export async function snapshotFigureScripts(fragment: string, declarations: Declarations) {
  if (!/<script\s[^>]*\bsrc\s*=/i.test(fragment)) return fragment;
  if (Object.values(declarations).some(binding => binding.type !== 'image')) return fragment;
  const template = document.createElement('template');
  template.innerHTML = fragment;
  const deadline = performance.now() + loadTimeout;
  const encoder = new TextEncoder();
  let remaining = maxSourceBytes - encoder.encode(fragment).byteLength;
  const fetched = new Map<string, string>();
  for (const tag of template.content.querySelectorAll<HTMLScriptElement>('script[src]')) {
    const src = tag.getAttribute('src')!;
    if (!src.startsWith('https://') || bundledUrls.has(src) || (tag.type && !scriptMime.test(tag.type))) continue;
    const url = new URL(src);
    if (url.username || url.password || !scriptOrigins.has(url.origin)) continue;
    const key = JSON.stringify([src, tag.integrity]);
    let data = fetched.get(key);
    if (!data) {
      const timeout = deadline - performance.now();
      if (timeout <= 0) throw Error('Timed out loading figure dependencies.');
      data = await fetchFigureAsset(src, scriptMime, remaining + src.length, tag.integrity || undefined, timeout);
      fetched.set(key, data);
    }
    remaining -= data.length - src.length;
    if (remaining < 0) throw Error('The figure and its embedded dependencies exceed 1 MiB.');
    tag.setAttribute('src', data);
  }
  return fetched.size ? template.innerHTML : fragment;
}
