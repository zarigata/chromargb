import { useEffect, useRef } from 'react';
import { drawFrame } from '../effectPreview';
import type { EffectKind, EffectParams } from '../types';

/** Animated LED-strip canvas approximating an effect client-side. */
export function StripPreview({
  effect,
  params,
  leds = 24,
  height = 26,
}: {
  effect: EffectKind;
  params: EffectParams;
  leds?: number;
  height?: number;
}) {
  const ref = useRef<HTMLCanvasElement>(null);
  const p = useRef(params);
  p.current = params; // live params without restarting the animation clock

  useEffect(() => {
    const canvas = ref.current;
    if (!canvas) return;
    const ctx = canvas.getContext('2d');
    if (!ctx) return;
    const dpr = window.devicePixelRatio || 1;
    let raf = 0;
    const t0 = performance.now();

    const render = () => {
      const w = canvas.clientWidth;
      const h = height;
      if (canvas.width !== Math.round(w * dpr) || canvas.height !== Math.round(h * dpr)) {
        canvas.width = Math.round(w * dpr);
        canvas.height = Math.round(h * dpr);
      }
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      ctx.clearRect(0, 0, w, h);
      const colors = drawFrame(effect, p.current, leds, (performance.now() - t0) / 1000);
      const gap = 2;
      const cell = Math.max(1, (w - gap * (leds - 1)) / leds);
      for (let i = 0; i < leds; i++) {
        const c = colors[i];
        ctx.fillStyle = `rgb(${c[0]},${c[1]},${c[2]})`;
        ctx.shadowColor = ctx.fillStyle;
        ctx.shadowBlur = 4;
        ctx.fillRect(i * (cell + gap), 2, cell, h - 4);
      }
      ctx.shadowBlur = 0;
      raf = requestAnimationFrame(render);
    };
    raf = requestAnimationFrame(render);
    return () => cancelAnimationFrame(raf);
  }, [effect, leds, height]);

  return <canvas ref={ref} className="strip" style={{ height }} />;
}
