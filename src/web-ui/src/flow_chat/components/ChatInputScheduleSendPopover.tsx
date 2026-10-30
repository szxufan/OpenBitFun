/**
 * Schedule-send popover for the composer.
 *
 * Offers configurable quick shortcuts (from `app.flow_chat.schedule_send_presets`)
 * plus a custom date-time entry. Confirming hands the chosen local time back to
 * the caller, which creates a one-shot scheduled job on the current session.
 */

import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { Button, createOverlayPortal, useDismissibleLayer } from '@openbitfun/ui';

import { getAppearanceOverlayHost } from '@/infrastructure/appearance/runtime/AppearanceOverlayHost';
import { useI18n } from '@/infrastructure/i18n';
import { useAnchoredPopoverPosition } from '@/shared/utils/useAnchoredPopoverPosition';
import { useScheduleSendPresets } from '@/infrastructure/config/hooks/useScheduleSendPresets';
import { resolvePresetDate } from '@/infrastructure/config/services/ScheduleSendPresets';
import type { ScheduleSendPreset } from '@/infrastructure/config/types';
import LocalizedDateTimeField from '@/app/components/scheduled-jobs/LocalizedDateTimeField';

export interface ChatInputScheduleSendPopoverProps {
  anchorRef: React.RefObject<HTMLElement | null>;
  triggerRef: React.RefObject<HTMLElement | null>;
  /** Truncated draft text so the user confirms what will be sent. */
  draftPreview: string;
  onConfirm: (date: Date) => void;
  onClose: () => void;
}

function toLocalInputValue(date: Date): string {
  const pad = (value: number) => String(value).padStart(2, '0');
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

const ChatInputScheduleSendPopover: React.FC<ChatInputScheduleSendPopoverProps> = ({
  anchorRef,
  triggerRef,
  draftPreview,
  onConfirm,
  onClose,
}) => {
  const { t, formatDate } = useI18n('flow-chat');
  const popoverRef = useRef<HTMLDivElement | null>(null);
  const { presets, error } = useScheduleSendPresets();

  const options = useMemo(() => {
    const now = new Date();
    return (presets ?? [])
      .map((preset: ScheduleSendPreset) => {
        const date = resolvePresetDate(preset, now);
        if (!date || date.getTime() <= now.getTime()) return null;
        const label = preset.kind === 'relative'
          ? t('input.scheduleSendPresetRelative', { minutes: preset.offsetMinutes })
          : preset.kind === 'daily'
            ? t('input.scheduleSendPresetDaily', { time: preset.time })
            : t('input.scheduleSendPresetTomorrow', { time: preset.time });
        const token = 'offsetMinutes' in preset ? preset.offsetMinutes : preset.time;
        return { key: `${preset.kind}:${token}`, date, label };
      })
      .filter((option): option is { key: string; date: Date; label: string } => option !== null);
  }, [presets, t]);

  const [selectedDate, setSelectedDate] = useState<Date | null>(options[0]?.date ?? null);
  const [customValue, setCustomValue] = useState<string>(() => (options[0] ? toLocalInputValue(options[0].date) : ''));
  const [hasUserSelection, setHasUserSelection] = useState(false);

  // Presets load asynchronously; the initial state above sees an empty list
  // when the config cache is cold. Adopt the first option once it arrives,
  // unless the user has already picked something or cleared the field.
  useEffect(() => {
    if (hasUserSelection || selectedDate !== null || options.length === 0) return;
    setSelectedDate(options[0].date);
    setCustomValue(toLocalInputValue(options[0].date));
  }, [hasUserSelection, options, selectedDate]);

  const layout = useAnchoredPopoverPosition({
    open: true,
    anchorRef,
    popoverRef,
    preferredPlacement: 'top',
    alignment: 'end',
    gap: 6,
    layoutRevision: options.length,
  });

  const branches = useMemo(() => [anchorRef], [anchorRef]);
  useDismissibleLayer({
    enabled: true,
    layerRef: popoverRef,
    branchRefs: branches,
    onDismiss: reason => {
      onClose();
      if (reason === 'escape-key') triggerRef.current?.focus();
    },
  });

  const selectOption = useCallback((date: Date) => {
    setHasUserSelection(true);
    setSelectedDate(date);
    setCustomValue(toLocalInputValue(date));
  }, []);

  const handleCustomChange = useCallback((value: string) => {
    setHasUserSelection(true);
    setCustomValue(value);
    if (!value.trim()) {
      setSelectedDate(null);
      return;
    }
    const parsed = new Date(value);
    setSelectedDate(Number.isNaN(parsed.getTime()) ? null : parsed);
  }, []);

  const now = Date.now();
  const canConfirm = selectedDate !== null && selectedDate.getTime() > now;

  const handleConfirm = useCallback(() => {
    if (!selectedDate || selectedDate.getTime() <= now) return;
    onConfirm(selectedDate);
  }, [now, onConfirm, selectedDate]);

  return createOverlayPortal(
    <div
      ref={popoverRef}
      className="openbitfun-chat-input__schedule-popover"
      data-openbitfun-component="chat-input"
      data-openbitfun-part="scheduleSendPopover"
      data-openbitfun-state="open"
      data-testid="chat-input-schedule-popover"
      role="dialog"
      aria-label={t('input.scheduleSendTitle')}
      style={{
        top: `${layout?.top ?? 0}px`,
        left: `${layout?.left ?? 0}px`,
        visibility: layout ? 'visible' : 'hidden',
      }}
    >
      <div className="openbitfun-chat-input__schedule-preview" data-openbitfun-component="chat-input" data-openbitfun-part="schedulePreview" title={draftPreview}>
        {draftPreview}
      </div>

      <div className="openbitfun-chat-input__schedule-section-label">{t('input.scheduleSendQuick')}</div>
      {error ? (
        <div className="openbitfun-chat-input__schedule-empty" data-openbitfun-state="error">{t('input.scheduleSendFailed')}</div>
      ) : presets === null ? (
        <div className="openbitfun-chat-input__schedule-empty" data-openbitfun-state="loading">{t('input.scheduleSendTitle')}</div>
      ) : options.length > 0 ? (
        <div className="openbitfun-chat-input__schedule-options" role="listbox" aria-label={t('input.scheduleSendQuick')}>
          {options.map(option => (
            <button
              key={option.key}
              type="button"
              role="option"
              aria-selected={selectedDate?.getTime() === option.date.getTime()}
              className={`openbitfun-chat-input__schedule-option${selectedDate?.getTime() === option.date.getTime() ? ' openbitfun-chat-input__schedule-option--selected' : ''}`}
              data-openbitfun-component="chat-input"
              data-openbitfun-part="scheduleOption"
              data-testid={`chat-input-schedule-option-${option.key}`}
              onClick={() => selectOption(option.date)}
            >
              <span className="openbitfun-chat-input__schedule-option-label">{option.label}</span>
              <span className="openbitfun-chat-input__schedule-option-time">
                {formatDate(option.date, { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' })}
              </span>
            </button>
          ))}
        </div>
      ) : null}

      <div className="openbitfun-chat-input__schedule-section-label">{t('input.scheduleSendCustom')}</div>
      <LocalizedDateTimeField
        value={customValue}
        onChange={handleCustomChange}
        aria-label={t('input.scheduleSendCustom')}
      />

      <div className="openbitfun-chat-input__schedule-foot">
        {!canConfirm && customValue.trim() ? (
          <span className="openbitfun-chat-input__schedule-hint">{t('input.scheduleSendInvalidTime')}</span>
        ) : null}
        <Button size="sm" variant="outline" onClick={onClose}>{t('input.cancelShortcut')}</Button>
        <Button size="sm" variant="fill" disabled={!canConfirm} data-testid="chat-input-schedule-confirm" onClick={handleConfirm}>
          {t('input.scheduleSendConfirm')}
        </Button>
      </div>
    </div>,
    getAppearanceOverlayHost(),
    undefined,
    { ownerRef: anchorRef },
  );
};

export default ChatInputScheduleSendPopover;
