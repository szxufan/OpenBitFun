/**
 * Skill tool display — compact row (same pattern as Read file).
 */

import React from 'react';
import { useTranslation } from 'react-i18next';
import {
  SkillToolCard,
  type FlowChatToolStatus,
} from '@openbitfun/ui/flow-chat';
import { getToolCardStatus, getToolCardStatusDescription } from './toolCardStatus';
import type { ToolCardProps } from '../types/flow-chat';
import { skillInvocationName } from '../utils/toolInvocationIdentity';

export const SkillDisplay: React.FC<ToolCardProps> = React.memo(({ toolItem }) => {
  const { t } = useTranslation('flow-chat');
  const { toolResult } = toolItem;
  const status = getToolCardStatus(toolItem);
  const displayName = skillInvocationName(toolItem)
    ?? (status === 'completed' ? t('toolCards.skill.unknownSkill') : t('toolCards.skill.unknown'));

  return (
    <SkillToolCard
      status={status as FlowChatToolStatus}
      action={t('toolCards.skill.action')}
      summary={displayName}
      statusDescription={getToolCardStatusDescription(status, t, toolResult?.error)}
    />
  );
});
