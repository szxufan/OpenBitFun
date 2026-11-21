export const visualArtifactRoot = 'openbitfun://current-session/artifacts/visualizations/';

export const visualArtifactUri =
  /^openbitfun:\/\/current-session\/artifacts\/visualizations\/(?:v2\/)?[a-z0-9][a-z0-9-]{0,95}\.html$/;

export function isCodexVisualUri(uri: string) {
  return visualArtifactUri.test(uri) && uri.includes('/visualizations/v2/');
}

/** The producing host can attest a local session path while preserving the requested file_path. */
export function visualReceiptPath(result?: { file_path?: unknown; visualization_uri?: unknown }) {
  const uri = result?.visualization_uri;
  return typeof uri === 'string' && isCodexVisualUri(uri) ? uri : result?.file_path;
}

const marker = 'visualize';
const damagedMarker = '㈀visualize㈁';

function figureReference(path: string, title: unknown, wide = false) {
  const label = typeof title === 'string' && title.trim() && !title.includes('openbitfun:')
    ? title : path.split('/').pop()!.slice(0, -5);
  return { path, title: label, wide };
}

export function readVisualReference(text: string) {
  const link = /^\[([^\]\n]+)\]\(([^()\s]+)\)$/.exec(text);
  if (link && visualArtifactUri.test(link[2])) return figureReference(link[2], link[1]);
  if (!text.startsWith(marker) || !text.endsWith('')) return;
  try {
    const ref = JSON.parse(text.slice(marker.length, -1));
    if (typeof ref?.path === 'string' && isCodexVisualUri(ref.path)) {
      return figureReference(ref.path, ref.title, ref.mode === 'wide');
    }
  } catch {
    // Incomplete and malformed model output can still have a checked receipt.
  }
}

// A file beside a v2 figure is an internal data source, including nested CSVs.
export const visualSidecarUri =
  /^openbitfun:\/\/current-session\/artifacts\/visualizations\/v2\/(?:[\w-][\w.-]*\/)*[\w-][\w.-]*$/;

type Reference = NonNullable<ReturnType<typeof readVisualReference>>;

/** Keep tool protocol out of the Markdown/typewriter surface. Only well-formed
 * references become figures; a damaged reference falls back to the turn receipt. */
export function visualReplyParts(text: string): (string | Reference)[] {
  if (!text.includes(marker[0]) && !text.includes(damagedMarker[0]) && !text.includes('openbitfun:')) return [text];

  const parts: (string | Reference)[] = [];
  let lines: string[] = [];
  const flush = () => {
    if (lines.length) parts.push(lines.join('\n'));
    lines = [];
  };
  let fence = '';
  for (const line of text.split('\n')) {
    const delimiter = /^ {0,3}(`{3,}|~{3,})(.*)$/.exec(line);
    if (delimiter) {
      if (!fence) fence = delimiter[1];
      else if (delimiter[1].startsWith(fence) && !delimiter[2].trim()) fence = '';
    }
    const trimmed = line.trim();
    // A marker may arrive one character at a time, or end with invalid JSON.
    const ref = !fence && readVisualReference(trimmed);
    if (ref) {
      flush();
      parts.push(ref);
      continue;
    }
    if (trimmed && (marker.startsWith(trimmed) || damagedMarker.startsWith(trimmed) ||
        /^[㈀]visualize/.test(trimmed))) continue;
    if (/^(?:[^\w\s]*visualize\W.*|\{\s*"path"\s*:\s*")openbitfun:/i.test(trimmed)) continue;
    // Mask bare or unfinished reserved URLs even when they occur in prose.
    // Links to other session resources still belong to the normal renderer.
    lines.push(line.replace(/openbitfun:[A-Za-z0-9_./%:?=#&+-]*/g, (uri, offset) => {
      if (!visualArtifactRoot.startsWith(uri) && !uri.startsWith(visualArtifactRoot)) return uri;
      const labeledLink = !fence && /\]\(<?$/.test(line.slice(0, offset))
        && /^>?(?:\s+["'][^"']*["'])?\)/.test(line.slice(offset + uri.length));
      return labeledLink && visualArtifactUri.test(uri) ? uri : '';
    }));
  }
  flush();
  return parts;
}
