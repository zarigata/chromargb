import type { EffectKind, EffectParams, LayerSpec } from './types';

/** Sensible default params per effect kind (also the studio's starting point). */
export function defaultParams(effect: EffectKind): EffectParams {
  switch (effect) {
    case 'static': return { color: [0, 255, 68] };
    case 'wave': return { speed: 60, direction: 'right' };
    case 'spectrum': return { speed: 50 };
    case 'breathe': return { speed: 40, color: [0, 120, 255] };
    case 'gradient': return { colors: [[255, 0, 68], [0, 86, 255], [128, 0, 255]] };
    case 'ripple': return { color: [124, 92, 255], speed: 60 };
    case 'reactive': return { color: [0, 224, 124], fade: 50 };
    case 'starlight': return { colors: [[255, 255, 255], [124, 92, 255], [0, 224, 124]], speed: 50 };
    case 'fire': return { speed: 60 };
    case 'audio_meter': return { color: [0, 224, 124], bands: 12 };
  }
}

export function cloneParams(p: EffectParams): EffectParams {
  return JSON.parse(JSON.stringify(p)) as EffectParams;
}

export function makeLayer(effect: EffectKind, over: Partial<LayerSpec> = {}): LayerSpec {
  return {
    effect,
    params: defaultParams(effect),
    opacity: 100,
    blend: 'normal',
    enabled: true,
    ...over,
  };
}
