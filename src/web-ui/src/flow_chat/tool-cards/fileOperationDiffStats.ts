import { diffLines } from 'diff';
import { splitFilePathAndContent } from '@/shared/utils/partialJsonParser';
import type { FlowToolItem } from '../types/flow-chat';
import { projectEffectiveToolItem } from '../utils/toolInvocationIdentity';
import { getToolCardStatus } from './toolCardStatus';

export interface FileOperationDiffStats {
  additions: number;
  deletions: number;
}

export const EMPTY_FILE_DIFF_STATS: FileOperationDiffStats = { additions: 0, deletions: 0 };

export function fileOperationPreview(item: FlowToolItem) {
  const tool = projectEffectiveToolItem(item);
  const params = tool.partialParams || tool.toolCall?.input;
  const string = (value: unknown) => typeof value === 'string' ? value : '';
  return {
    oldStringContent: string(params?.old_string),
    newStringContent: string(params?.new_string),
    contentPreview: splitFilePathAndContent(params?.payload)?.content
      ?? (typeof params?.payload === 'string' ? params.payload : string(params?.content) || string(params?.contents)),
  };
}

const MAX_STREAMING_EXACT_DIFF_LINES = 80;
const MAX_INLINE_DIFF_CHARS = 100_000;

function countContentLines(text: string): number {
  if (!text) return 0;
  let lines = 1;
  for (let i = 0; i < text.length; i += 1) {
    if (text.charCodeAt(i) === 10) {
      lines += 1;
    }
  }
  return text.charCodeAt(text.length - 1) === 10 ? lines - 1 : lines;
}

/** The same per-operation fallback used by a native card and its collection. */
export function localFileOperationDiffStats(item: FlowToolItem): FileOperationDiffStats {
  const tool = projectEffectiveToolItem(item);
  if (getToolCardStatus(tool) === 'error') return EMPTY_FILE_DIFF_STATS;
  const { oldStringContent, newStringContent, contentPreview } = fileOperationPreview(tool);
  if (tool.toolName === 'Write' && contentPreview) {
    return { additions: countContentLines(contentPreview), deletions: 0 };
  }
  if (tool.toolName === 'Edit' && (oldStringContent || newStringContent)) {
    const oldLines = countContentLines(oldStringContent);
    const newLines = countContentLines(newStringContent);
    if (
      (tool.isParamsStreaming && oldLines + newLines > MAX_STREAMING_EXACT_DIFF_LINES) ||
      oldStringContent.length + newStringContent.length > MAX_INLINE_DIFF_CHARS
    ) {
      return { additions: newLines, deletions: oldLines };
    }
    const stats = { additions: 0, deletions: 0 };
    for (const change of diffLines(oldStringContent, newStringContent)) {
      if (change.added) stats.additions += change.count ?? 0;
      else if (change.removed) stats.deletions += change.count ?? 0;
    }
    return stats;
  }
  return EMPTY_FILE_DIFF_STATS;
}

export function sumFileOperationDiffStats(stats: Iterable<FileOperationDiffStats>): FileOperationDiffStats {
  const total = { additions: 0, deletions: 0 };
  for (const item of stats) {
    total.additions += item.additions;
    total.deletions += item.deletions;
  }
  return total;
}
