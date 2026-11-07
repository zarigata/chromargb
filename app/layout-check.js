// One-shot check: PHYSICAL_LAYOUT covers every razer KEYMAP cell, no unintended overlaps.
import { readFileSync } from 'node:fs';
import { PHYSICAL_LAYOUT } from './src/keyboardLayout.ts';

const keymap = readFileSync('../core/chromargb-drivers/src/razer/mod.rs', 'utf8');
const cells = {};
for (const m of keymap.matchAll(/\(0x[0-9A-Fa-f]+,\s*(\d+),\s*(\d+)\)/g)) cells[m[1] + ':' + m[2]] = true;

const entries = PHYSICAL_LAYOUT;
const mine = new Set(entries.map(([r, c]) => r + ':' + c));
const missing = Object.keys(cells).filter((k) => !mine.has(k));
const extra = [...mine].filter((k) => !cells[k]);
let overlaps = [];
for (const row of [0, 1, 2, 3, 4, 5]) {
  const rects = entries.filter(([r]) => r === row).sort((a, b) => a[2] - b[2]);
  for (let i = 1; i < rects.length; i++)
    if (rects[i][2] < rects[i - 1][2] + rects[i - 1][3] - 0.01 && rects[i][4] !== '+')
      overlaps.push(`row ${row}: ${rects[i][4]} over ${rects[i - 1][4]}`);
}
const ok = entries.length === Object.keys(cells).length && missing.length === 0 && extra.length === 0 && overlaps.length === 0;
console.log(`entries=${entries.length} keymap=${Object.keys(cells).length} missing=[${missing}] extra=[${extra}] overlaps=[${overlaps}]`);
console.log(ok ? 'LAYOUT OK' : 'LAYOUT BAD');
process.exit(ok ? 0 : 1);
