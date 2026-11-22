import path from 'path-browserify';
import type { LineRange } from '@/shared/editor/LineRange';

export const COMPUTER_LINK_PREFIX = 'computer://';
export const FILE_LINK_PREFIX = 'file://';
export const WORKSPACE_FOLDER_PLACEHOLDER = '{{workspaceFolder}}';

/** A filesystem destination that can be handled by the owning workspace. */
export function isMarkdownFileHref(href: string): boolean {
  if (!href || href !== href.trim() || /\p{Cc}/u.test(href)) return false;
  if (/^[?#]|^[\\/]{2}/.test(href)) return false;
  return href.startsWith('file:') || href.startsWith(COMPUTER_LINK_PREFIX)
    || /^[a-z]:[\\/]/i.test(href) || !/^[^/\\?#]*:/.test(href);
}

export function normalizeFileLikeHref(rawHref: string): string {
  if (/^openbitfun:\/\/(?:runtime|current-session)\//.test(rawHref)) return rawHref;
  let filePath = rawHref;

  if (rawHref.startsWith(COMPUTER_LINK_PREFIX)) {
    filePath = rawHref.slice(COMPUTER_LINK_PREFIX.length);
  } else if (rawHref.startsWith(FILE_LINK_PREFIX)) {
    filePath = rawHref.slice(FILE_LINK_PREFIX.length);
  } else if (rawHref.startsWith('file:')) {
    filePath = rawHref.slice('file:'.length);
  }

  if (filePath.startsWith(WORKSPACE_FOLDER_PLACEHOLDER)) {
    filePath = filePath.slice(WORKSPACE_FOLDER_PLACEHOLDER.length);
    if (filePath.startsWith('/')) {
      filePath = filePath.slice(1);
    }
  }

  // Normalize URI-style Windows drive paths to native absolute paths.
  if (/^\/{2,}[A-Za-z]:[\\/]/.test(filePath)) {
    filePath = filePath.replace(/^\/+/, '/');
  }
  if (/^\/[A-Za-z]:[\\/]/.test(filePath)) {
    filePath = filePath.slice(1);
  }

  try {
    return decodeURIComponent(filePath);
  } catch {
    return filePath;
  }
}

export function isAbsoluteFilesystemPath(filePath: string): boolean {
  const normalized = filePath.replace(/\\/g, '/');
  return /^\/?[A-Za-z]:/.test(normalized) || (normalized.startsWith('/') && !normalized.startsWith('//'));
}

export function resolveBaseRelativePath(targetPath: string, basePath?: string): string {
  if (/^openbitfun:\/\/(?:runtime|current-session)\//.test(targetPath)) return targetPath;
  if (!targetPath || !basePath || isAbsoluteFilesystemPath(targetPath)) return targetPath;
  return path.posix.join(basePath, targetPath.replace(/\\/g, '/'));
}

export function parseMarkdownLineRange(hash: string): LineRange | undefined {
  const cleanHash = hash.replace(/^#/, '');
  const match = cleanHash.match(/^L(\d+)(?:-L?(\d+))?$/i) ?? cleanHash.match(/^(\d+)(?:-(\d+))?$/);
  if (!match) return undefined;
  const [, start, end] = match;
  return { start: parseInt(start, 10), end: end ? parseInt(end, 10) : undefined };
}
