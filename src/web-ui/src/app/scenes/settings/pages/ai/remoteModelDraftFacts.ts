import type { ModelCategory, ReasoningConfig, ReasoningPreset } from '@/infrastructure/config/types';

/** Metadata fields a richer /models endpoint may advertise per model. */
export interface RemoteModelDraftFacts {
  context_length?: number;
  max_output_tokens?: number;
  reasoning_options?: string[];
  supported_endpoint_types?: string[];
  support_image_input?: boolean;
  support_video_input?: boolean;
}

const IMAGE_ENDPOINT_TYPES = new Set(['image-generation', 'image-edit']);
const CHAT_ENDPOINT_TYPES = new Set(['openai', 'responses']);

/**
 * Map a remote /models entry onto a supported model category. Only confident
 * facts are translated: image endpoints become image generation, advertised
 * image/video input becomes multimodal, and a chat-only endpoint stays general
 * chat. Unknown or missing metadata leaves the caller's default in place.
 */
export function remoteModelCategory(option?: RemoteModelDraftFacts): ModelCategory | undefined {
  if (!option) return undefined;
  const endpoints = (option.supported_endpoint_types ?? [])
    .map(type => type.trim().toLowerCase().replace(/_/g, '-'));
  if (endpoints.some(type => IMAGE_ENDPOINT_TYPES.has(type))) {
    return 'image_generation';
  }
  if (option.support_image_input || option.support_video_input) {
    return 'multimodal';
  }
  if (endpoints.length > 0 && endpoints.every(type => CHAT_ENDPOINT_TYPES.has(type))) {
    return 'general_chat';
  }
  return undefined;
}

/** Build effort presets from provider-advertised reasoning levels. */
export function remoteModelReasoning(option?: RemoteModelDraftFacts): ReasoningConfig | undefined {
  const levels = Array.from(new Set(
    (option?.reasoning_options ?? [])
      .map(value => value.trim().toLowerCase())
      .filter(Boolean),
  ));
  if (levels.length === 0) return undefined;
  const presets: ReasoningPreset[] = levels.map((level, index) => ({
    id: level,
    label: level,
    order: index,
    actions: [{ type: 'effort', value: level }],
  }));
  return {
    catalog: { source: 'auto' },
    default_preset: levels.includes('medium') ? 'medium' : undefined,
    presets,
  };
}
