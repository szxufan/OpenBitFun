import './ImageGenerationSection.scss';
import { Card, CardFooter, CardHeader, OverflowText, StatusPill } from '@openbitfun/ui';
import { useI18n } from '@/infrastructure/i18n';
import type { ReactNode } from 'react';
import type { ImageGenerationSettings } from './useImageGenerationSettings';
import type { ImageServiceConfig } from '../../types';
import { findImageDialectPreset } from './imageDialects';
import { imageModelLabel } from './imageModels';

type Props = { settings: ImageGenerationSettings; service: ImageServiceConfig; actions: ReactNode };

/**
 * Legacy standalone image service card. Creation and editing retired when image
 * models attached to providers; existing records stay listed (via the pool
 * filters) so users can disable or remove them while they migrate.
 */
export function ImageGenerationModelCard({ settings, service, actions }: Props) {
  const { t } = useI18n('settings/media');
  const label = imageModelLabel(service);
  return <div className="openbitfun-model-settings__pool-item" data-openbitfun-component="model-settings"
    data-openbitfun-part="modelItem" data-openbitfun-state={!service.enabled ? 'disabled' : undefined}
    data-testid="settings-image-model-row" data-model-id={service.id}>
    <Card appearance="subtle" padding="md" gap="sm" className="openbitfun-model-settings__pool-card">
      <CardHeader className="openbitfun-model-settings__pool-card-header" contentAlign="start" title={
        <div className="openbitfun-model-settings__pool-card-title"><OverflowText>{label}</OverflowText>
        </div>} description={
          <div className="openbitfun-model-settings__pool-tags"><StatusPill tone="neutral">{t('legacy.badge')}</StatusPill>
            {settings.selectedId === service.id && <StatusPill tone="neutral">{t('card.defaultBadge')}</StatusPill>}
          </div>} />
      <CardFooter align="between" className="openbitfun-model-settings__pool-card-footer">
        <OverflowText className="openbitfun-model-settings__pool-card-source">{t(findImageDialectPreset(service.dialect).labelKey)}</OverflowText>
        {actions}
      </CardFooter>
    </Card>
  </div>;
}
