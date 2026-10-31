import type { AIModelConfig, ImageDialect, ImageServiceConfig, MediaConfig } from '../../types';
import { getEffectiveModelCapabilities } from '../../services/modelCategory';
import { stableJson } from '../modelConnectionTestPlan';
import { resolveImageEndpoint, withImageServiceAuth } from './imageDialects';

/** Compare request behavior, not the generated record id or display name. */
export function sameImageService(left: ImageServiceConfig, right: ImageServiceConfig): boolean {
  const comparable = (value: ImageServiceConfig) => {
    const service = withImageServiceAuth(value, value.auth ?? { type: 'api_key' });
    const params = Object.fromEntries(Object.entries(service.params ?? {}).filter(([, entry]) => entry !== undefined && entry !== ''));
    return {
      dialect: service.dialect,
      endpoint: resolveImageEndpoint(service.base_url, service.request_url, service.dialect).replace(/\/+$/, ''),
      model: service.model_name.trim(),
      auth: service.auth,
      apiKey: service.auth?.type === 'api_key' ? service.api_key.trim() : '',
      headers: service.custom_headers ?? {},
      headersMode: service.custom_headers_mode ?? 'merge',
      skipSslVerify: service.skip_ssl_verify ?? false,
      params: { ...params, n: params.n ?? 1 },
    };
  };
  // Credentials are compared in memory only; never use this value as a DOM id or log it.
  return stableJson(comparable(left)) === stableJson(comparable(right));
}

export function duplicateImageService(services: readonly ImageServiceConfig[], draft: ImageServiceConfig): ImageServiceConfig | undefined {
  const original = services.find(service => service.id === draft.id);
  // Existing duplicate records remain editable; only adding or changing identity can introduce a duplicate.
  if (original && sameImageService(original, draft)) return undefined;
  return services.find(service => service.id !== draft.id && sameImageService(service, draft));
}

export class DuplicateImageServiceError extends Error {
  constructor() { super('An identical image service is already configured'); }
}

export function normalizeMedia(value: MediaConfig | null | undefined): MediaConfig {
  return { ...value, image_services: value?.image_services ?? [] };
}

export function effectiveImageServiceId(media: MediaConfig): string {
  const enabled = media.image_services.filter(service => service.enabled);
  if (enabled.some(service => service.id === media.default_image_service)) return media.default_image_service!;
  return enabled[0]?.id ?? '';
}

export function withDefaultImageService(media: MediaConfig): MediaConfig {
  const id = effectiveImageServiceId(media);
  return id && id !== media.default_image_service ? { ...media, default_image_service: id } : media;
}

export function imageModelLabel(service: ImageServiceConfig): string {
  if (service.name && service.model_name && service.name !== service.model_name) return `${service.name} · ${service.model_name}`;
  return service.name || service.model_name || 'ChatGPT';
}

/**
 * Configured models that can drive image generation: Codex subscription chats
 * (built-in image tool) and models the user explicitly categorized as image
 * generation (their endpoint facts come from the chat-model record).
 */
export function configuredSubscriptionImageModels(models: readonly AIModelConfig[]): AIModelConfig[] {
  return models.filter(model => model.enabled && model.id && (
    (model.auth?.type === 'subscription' && model.auth.provider === 'codex')
    || getEffectiveModelCapabilities(model).includes('image_generation')
  ));
}

/** DashScope hosts serve images on the native dialect, everything else on OpenAI images. */
function imageDialectForConfiguredModel(model: AIModelConfig): ImageDialect {
  const host = `${model.base_url ?? ''} ${model.request_url ?? ''}`.toLowerCase();
  if (host.includes('dashscope') || host.includes('aliyuncs.com')) return 'dashscope_native';
  return 'openai_images';
}

/** Re-root a chat endpoint onto the dialect's own API root when possible. */
function imageBaseFromModelBaseUrl(baseUrl: string, dialect: ImageDialect): string {
  const root = baseUrl.trim().replace(/\/+$/, '');
  if (dialect === 'dashscope_native') {
    // The OpenAI-compatible-mode chat root cannot serve the native image
    // endpoint; rebuild the native root on the same host.
    const compatible = root.match(/^(https?:\/\/[^/]+dashscope[^/]*)\/compatible-mode/);
    if (compatible) return `${compatible[1]}/api/v1`;
    return root.endsWith('/api/v1') || root.endsWith('/services/aigc/multimodal-generation/generation')
      ? root : `${root}/api/v1`;
  }
  return root.endsWith('/v1') || root.endsWith('/images/generations') ? root : `${root}/v1`;
}

export function imageServiceFromSubscriptionModel(model: AIModelConfig): ImageServiceConfig {
  if (model.auth?.type === 'subscription' && model.auth.provider === 'codex') {
    return {
      id: `configured-image:${model.id}`, name: model.name || model.model_name,
      dialect: 'chatgpt_codex', base_url: 'https://chatgpt.com/backend-api/codex',
      model_name: model.model_name, api_key: '', auth: { ...model.auth }, enabled: true,
      params: { n: 1 }, skip_ssl_verify: model.skip_ssl_verify,
    };
  }
  if (!getEffectiveModelCapabilities(model).includes('image_generation') || !model.base_url?.trim()) {
    throw new Error('This configured model does not support subscription image generation');
  }
  const dialect = model.image_dialect ?? imageDialectForConfiguredModel(model);
  return {
    id: `configured-image:${model.id}`, name: model.name || model.model_name,
    dialect, base_url: imageBaseFromModelBaseUrl(model.base_url ?? '', dialect), model_name: model.model_name,
    api_key: model.api_key ?? '', auth: model.auth ?? { type: 'api_key' }, enabled: true,
    custom_headers: model.custom_headers, custom_headers_mode: model.custom_headers_mode,
    skip_ssl_verify: model.skip_ssl_verify, params: { n: 1 },
  };
}

export function matchesImageModel(service: ImageServiceConfig, query: string, capability: string, provider: string): boolean {
  if (capability && capability !== 'image_generation') return false;
  if (provider && provider !== `image:${service.dialect}`) return false;
  const search = query.trim().toLowerCase();
  return !search || [service.name, service.model_name, service.dialect].join(' ').toLowerCase().includes(search);
}
