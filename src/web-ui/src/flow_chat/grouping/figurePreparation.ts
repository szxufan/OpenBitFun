import type { FlowToolItem, ModelRound } from '../types/flow-chat';
import { projectEffectiveToolItem, skillInvocationName } from '../utils/toolInvocationIdentity';
import { awaitsApproval, isShellActivity } from './activityClassification';
import { fileEditTarget } from './fileEdits';
import { classifyFlowGroupItem } from './policies';
import { buildFlowItemGroups, getModelRoundActiveItems, isModelRoundGroupingDisabled } from './roundGroups';

function isScratchFile(tool: FlowToolItem, workspace: string | undefined) {
  const path = fileEditTarget(tool);
  if (!path || !workspace) return false;
  const root = workspace.replace(/\\/g, '/').replace(/\/$/, '');
  if (path === root || path.startsWith(root + '/')) return false;
  return (
    /^\/(?:private\/)?(?:tmp\/|var\/folders\/[^/]+\/[^/]+\/T\/)/.test(path) ||
    /^[A-Za-z]:\/Users\/[^/]+\/AppData\/Local\/Temp\//i.test(path)
  );
}

/** Scoped to one turn segment. Steering and other skills start a new segment. */
export function figurePreparation(workspace?: string) {
  let active = false;
  return (round: ModelRound) => {
    const items = getModelRoundActiveItems(round);
    const members = new Set<string>();
    for (const item of items) {
      if (item.type !== 'tool' || item.subagentSessionId) continue;
      const tool = projectEffectiveToolItem(item as FlowToolItem);
      if (tool.importedSource) continue;
      if (awaitsApproval(tool) || ['rejected', 'cancelled', 'error'].includes(tool.status)
          || tool.toolResult?.success === false) {
        if (tool.toolName === 'Skill') active = false;
        continue;
      }
      if (tool.toolName === 'Skill') active = skillInvocationName(tool) === 'visualize';
      if (!active) continue;
      if (
        tool.toolName === 'Skill' ||
        isShellActivity(tool) ||
        classifyFlowGroupItem(tool) === 'explore' ||
        tool.toolName === 'view_image' ||
        isScratchFile(tool, workspace)
      ) {
        members.add(item.id);
      } else if (!tool.isParamsStreaming && !tool.toolResult?.result?.visualization_uri) {
        // Preserve ordinary project writes and their subsequent workflow.
        active = false;
      }
    }
    if (!members.size || isModelRoundGroupingDisabled(round)) return undefined;
    return buildFlowItemGroups({ items, retainCandidates: true, figureMembers: members });
  };
}
