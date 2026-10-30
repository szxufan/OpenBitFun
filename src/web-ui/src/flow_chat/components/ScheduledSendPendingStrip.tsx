/**
 * Pending scheduled-send strip.
 *
 * Lists the one-shot (`at`) scheduled jobs targeting the current session that
 * have not fired yet, so the user can see and cancel a scheduled message from
 * the composer. Triggered jobs are intentionally hidden: the backend keeps a
 * fired one-shot as a disabled record, so filtering on `lastEnqueuedAtMs` avoids
 * stale rows.
 */

import React, { useCallback, useEffect, useMemo, useState } from 'react';
import { Icon } from '@openbitfun/ui';
import { Clock, X } from 'lucide-react';

import { cronAPI, type CronJob } from '@/infrastructure/api';
import { useI18n } from '@/infrastructure/i18n';
import { notificationService } from '@/shared/notification-system';
import { confirmDanger } from '@/infrastructure/confirm-dialog';
import { createLogger } from '@/shared/utils/logger';

const log = createLogger('ScheduledSendPendingStrip');

export interface ScheduledSendPendingStripProps {
  sessionId: string | undefined;
}

function isPendingOneShot(job: CronJob): boolean {
  return job.schedule.kind === 'at'
    && job.enabled
    && !job.state.lastEnqueuedAtMs
    && !job.state.activeTurnId;
}

const ScheduledSendPendingStrip: React.FC<ScheduledSendPendingStripProps> = ({ sessionId }) => {
  const { t, formatDate } = useI18n('flow-chat');
  const [jobs, setJobs] = useState<CronJob[]>([]);

  const load = useCallback(async () => {
    if (!sessionId) {
      setJobs([]);
      return;
    }
    try {
      const list = await cronAPI.listJobs({ sessionId });
      setJobs(list.filter(isPendingOneShot));
    } catch (error) {
      log.warn('Failed to load pending scheduled sends', { sessionId, error });
    }
  }, [sessionId]);

  useEffect(() => {
    void load();
    // The event payload is only a hint; re-read the list like ScheduledJobsView.
    const unsubscribe = cronAPI.onJobsChanged(() => { void load(); });
    return () => { unsubscribe(); };
  }, [load]);

  const handleCancel = useCallback(async (job: CronJob) => {
    const confirmed = await confirmDanger(t('input.scheduledSendCancelConfirm'), null);
    if (!confirmed) return;
    try {
      await cronAPI.deleteJob(job.id);
      await load();
    } catch (error) {
      log.error('Failed to cancel scheduled send', { jobId: job.id, error });
      notificationService.error(t('input.scheduledSendCancelFailed'));
    }
  }, [load, t]);

  const rows = useMemo(() => jobs
    .map(job => {
      const at = job.schedule.kind === 'at' ? job.schedule.at : '';
      const date = new Date(at);
      return { job, date: Number.isNaN(date.getTime()) ? null : date };
    })
    .sort((left, right) => (left.date?.getTime() ?? 0) - (right.date?.getTime() ?? 0)), [jobs]);

  if (rows.length === 0) return null;

  return (
    <div
      className="openbitfun-chat-input__scheduled-strip"
      data-openbitfun-component="chat-input"
      data-openbitfun-part="scheduledSendStrip"
      data-testid="chat-input-scheduled-strip"
      aria-label={t('input.scheduledSendPendingTitle')}
    >
      <span className="openbitfun-chat-input__scheduled-strip-title">{t('input.scheduledSendPendingTitle')}</span>
      <ul className="openbitfun-chat-input__scheduled-strip-list">
        {rows.map(({ job, date }) => (
          <li key={job.id} className="openbitfun-chat-input__scheduled-item" data-openbitfun-component="chat-input" data-openbitfun-part="scheduledSendItem">
            <Icon glyph={Clock} size="sm" aria-hidden />
            <span className="openbitfun-chat-input__scheduled-time">
              {date ? formatDate(date, { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' }) : ''}
            </span>
            <span className="openbitfun-chat-input__scheduled-text" title={job.payload.text}>{job.payload.text}</span>
            <button
              type="button"
              className="openbitfun-chat-input__scheduled-cancel"
              aria-label={t('input.scheduledSendCancel')}
              data-testid={`chat-input-scheduled-cancel-${job.id}`}
              onClick={() => void handleCancel(job)}
            >
              <Icon glyph={X} size="sm" aria-hidden />
            </button>
          </li>
        ))}
      </ul>
    </div>
  );
};

export default ScheduledSendPendingStrip;
