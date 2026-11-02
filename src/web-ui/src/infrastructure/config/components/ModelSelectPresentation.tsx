import type { ComboboxOption } from '@openbitfun/ui';
import { formatContextLength, getProviderDisplayName } from '../services/modelConfigs';
import type { AIModelConfig } from '../types';
export type ModelSelectOption = ComboboxOption;

export function useModelSelectPresentation() {
  const formatContextWindow = (contextWindow?: number) => {
    if (!contextWindow) return null;
    return formatContextLength(contextWindow);
  };

  const buildModelOption = (model: AIModelConfig): ModelSelectOption => {
    const meta = [getProviderDisplayName(model)];
    const contextWindow = formatContextWindow(model.context_window);

    if (contextWindow) {
      meta.push(contextWindow);
    }
    return {
      description: meta.join(' · '),
      label: model.model_name || model.name || model.id || '',
      value: model.id || '',
    };
  };

  return { buildModelOption };
}
