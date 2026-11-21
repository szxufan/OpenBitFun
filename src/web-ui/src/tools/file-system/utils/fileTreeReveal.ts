import { repositoryPathKey } from '@/shared/utils/pathUtils';
import type { FlatFileNode } from '../types';

// Compiled once. The function is called for every reveal, and building these patterns per
// call showed up as a real share of the cost on long paths.
const WINDOWS_ABSOLUTE = /^[a-z]:[/\\]/i;
const WINDOWS_DRIVE_ONLY = /^[a-z]:$/i;
const TRAILING_SLASHES = /\/+$/;
const SLASHES = /\//g;
const BACKSLASHES = /\\/g;

function toForwardSlashes(path: string, windows: boolean): string {
  // A remote path is not Windows: its backslashes are literal name characters, so they stay.
  if (!windows || path.indexOf('\\') === -1) return path;
  return path.replace(BACKSLASHES, '/');
}

function stripTrailingSlashes(path: string): string {
  return path.charCodeAt(path.length - 1) === 47 ? path.replace(TRAILING_SLASHES, '') : path;
}

/** Plan ancestor expansion using the target filesystem's path semantics. */
export function planFileTreeReveal(root: string, target: string, remote = false): {
  pathsToExpand: string[]; targetPath: string;
} | null {
  if (!root || !target) return null;
  const windows = !remote && (WINDOWS_ABSOLUTE.test(root) || root.startsWith('\\\\'));
  const normalizedRoot = stripTrailingSlashes(toForwardSlashes(root, windows)) || '/';
  const normalizedTarget = stripTrailingSlashes(toForwardSlashes(target, windows)) || '/';
  // Lowercased copies are only for comparison: the returned paths keep the caller's casing.
  const comparedRoot = windows ? normalizedRoot.toLowerCase() : normalizedRoot;
  const comparedTarget = windows ? normalizedTarget.toLowerCase() : normalizedTarget;
  const prefix = normalizedRoot === '/' ? '/' : `${normalizedRoot}/`;
  const comparedPrefix = windows ? prefix.toLowerCase() : prefix;
  if (comparedTarget !== comparedRoot && !comparedTarget.startsWith(comparedPrefix)) return null;

  const parts = normalizedTarget.slice(normalizedRoot === '/' ? 1 : normalizedRoot.length).split('/');
  // Paths from a host are absolute. Do not let an unnormalized path escape the tree.
  for (const part of parts) {
    if (part === '.' || part === '..') return null;
  }
  const backslashOutput = windows && root.includes('\\');
  let currentPath = normalizedRoot;
  const pathsToExpand: string[] = [];
  for (const part of parts) {
    if (!part) continue;
    currentPath = `${currentPath === '/' ? '' : currentPath}/${part}`;
    const absolute = windows && WINDOWS_DRIVE_ONLY.test(currentPath) ? `${currentPath}/` : currentPath;
    pathsToExpand.push(backslashOutput ? absolute.replace(SLASHES, '\\') : absolute);
  }
  const targetPath = windows && WINDOWS_DRIVE_ONLY.test(currentPath) ? `${currentPath}/` : currentPath;
  return {
    pathsToExpand,
    targetPath: backslashOutput ? targetPath.replace(SLASHES, '\\') : targetPath,
  };
}

/** Address the data row, including compressed folders, rather than rendered DOM. */
export function findFileTreeRevealIndex(nodes: FlatFileNode[], target: string): number {
  const key = repositoryPathKey(target);
  return nodes.findIndex(node => repositoryPathKey(node.path) === key
    || node.originalNode?.originalNodes?.some(original => repositoryPathKey(original.path) === key));
}
