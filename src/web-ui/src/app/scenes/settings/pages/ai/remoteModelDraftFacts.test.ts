import { describe, expect, it } from 'vitest';
import { remoteModelCategory, remoteModelReasoning } from './remoteModelDraftFacts';

describe('remoteModelCategory', () => {
  it('maps image endpoints to the image generation category', () => {
    expect(remoteModelCategory({
      supported_endpoint_types: ['image-generation', 'image-edit'],
    })).toBe('image_generation');
    expect(remoteModelCategory({
      supported_endpoint_types: ['IMAGE_EDIT'],
    })).toBe('image_generation');
  });

  it('recognises multimodal input flags with or without endpoint types', () => {
    expect(remoteModelCategory({
      supported_endpoint_types: ['openai'],
      support_image_input: true,
    })).toBe('multimodal');
    expect(remoteModelCategory({
      support_video_input: true,
    })).toBe('multimodal');
  });

  it('keeps a chat-only endpoint as general chat', () => {
    expect(remoteModelCategory({ supported_endpoint_types: ['openai'] })).toBe('general_chat');
    expect(remoteModelCategory({ supported_endpoint_types: ['openai', 'responses'] })).toBe('general_chat');
  });

  it('leaves unknown service types to the caller default', () => {
    expect(remoteModelCategory({ supported_endpoint_types: ['jina-rerank'] })).toBeUndefined();
    expect(remoteModelCategory({ supported_endpoint_types: ['embeddings'] })).toBeUndefined();
    expect(remoteModelCategory({})).toBeUndefined();
    expect(remoteModelCategory(undefined)).toBeUndefined();
  });
});

describe('remoteModelReasoning', () => {
  it('builds one effort preset per advertised level', () => {
    const config = remoteModelReasoning({ reasoning_options: ['low', 'high', 'max'] });
    expect(config?.presets).toEqual([
      { id: 'low', label: 'low', order: 0, actions: [{ type: 'effort', value: 'low' }] },
      { id: 'high', label: 'high', order: 1, actions: [{ type: 'effort', value: 'high' }] },
      { id: 'max', label: 'max', order: 2, actions: [{ type: 'effort', value: 'max' }] },
    ]);
    // No advertised medium keeps the default selector on auto.
    expect(config?.default_preset).toBeUndefined();
    expect(config?.catalog).toEqual({ source: 'auto' });
  });

  it('defaults to medium when the provider advertises it', () => {
    const config = remoteModelReasoning({ reasoning_options: ['low', 'medium', 'high'] });
    expect(config?.default_preset).toBe('medium');
  });

  it('normalizes casing, trims, and deduplicates levels', () => {
    const config = remoteModelReasoning({ reasoning_options: [' Low ', 'low', 'HIGH', ''] });
    expect(config?.presets?.map(preset => preset.id)).toEqual(['low', 'high']);
  });

  it('returns undefined without advertised levels', () => {
    expect(remoteModelReasoning(undefined)).toBeUndefined();
    expect(remoteModelReasoning({})).toBeUndefined();
    expect(remoteModelReasoning({ reasoning_options: [] })).toBeUndefined();
    expect(remoteModelReasoning({ reasoning_options: ['  '] })).toBeUndefined();
  });
});
