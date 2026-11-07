// Headless smoke test: load the built renderer in a hidden Electron window and
// assert the daemon state. Modes:
//   mock    — mock-baked build: expect dot on + 3 cards (default)
//   offline — plain build with no daemon: expect dot off + offline banner
//   live    — plain build against the real daemon on ws://127.0.0.1:7474:
//             assert dot on, ≥1 card, each card's available state matches
//             list_devices, and drive set_color red → wave → stop → set_matrix
//             on the first matrix device, asserting each response ok:true.
//             Env: LIVE_DEVICE=<id> (default: first matrix-capable device).
// Usage: node_modules/.bin/electron smoke.cjs [rendererDir] [mock|offline|live]
const { app, BrowserWindow } = require('electron');
const path = require('node:path');

const dir = process.argv[2] || 'dist/renderer-mock';
const mode = process.argv[3] || 'mock';
const errors = [];

app.whenReady().then(() => {
  const win = new BrowserWindow({
    show: false,
    webPreferences: {
      // smoke-preload taps WebSocket so live mode can assert the frames the
      // React client itself exchanged (UI-driven, not harness-injected).
      // Isolation off so the preload's wrapped WebSocket applies to the page.
      preload: mode === 'live' ? path.join(__dirname, 'smoke-preload.cjs') : undefined,
      contextIsolation: false,
      sandbox: false,
    },
  });
  win.webContents.on('console-message', (_e, level, msg) => {
    // the CSP warning fires once per renderer in dev; anything else at warn+ is a failure
    if (level >= 2 && !/Security Warning|unsafe-eval|Content-Security/i.test(msg)) errors.push(msg);
  });
  win.webContents.on('pageerror', (_e, err) => errors.push(String(err)));

      win.webContents.on('did-finish-load', () => {
        setTimeout(async () => {
          const finish = (pass) => {
            const hold = Number(process.env.SMOKE_HOLD || 0);
            if (pass && (process.env.SMOKE_SHOT || hold)) {
              win.show();
              setTimeout(() => {
                if (!process.env.SMOKE_SHOT) {
                  app.exit(0);
                  return;
                }
                win.webContents.capturePage().then((img) => {
                  require('node:fs').writeFileSync(process.env.SMOKE_SHOT, img.toPNG());
                  console.log('SHOT saved:', process.env.SMOKE_SHOT);
                  app.exit(0);
                });
              }, hold * 1000 || 2000); // hold open for perf sampling; else let animation be mid-frame
              return;
            }
            app.exit(pass ? 0 : 1);
          };
          const dom = () =>
            win.webContents.executeJavaScript(
              `JSON.stringify({
                 dot: document.querySelector('.dot')?.className ?? 'none',
                 cards: document.querySelectorAll('.card').length,
                 banner: document.querySelector('.banner')?.textContent?.trim() ?? 'none',
                 buttons: document.querySelectorAll('.card-actions button').length,
                 names: [...document.querySelectorAll('.card-name')].map((e) => e.textContent.trim()),
                 offCards: document.querySelectorAll('.card.off').length,
                 notes: [...document.querySelectorAll('.card-note')].map((e) => e.textContent.trim()),
                 badge0: document.querySelector('.card .badge')?.textContent?.trim() ?? 'none',
               })`,
            ).then((j) => JSON.parse(j));

          const settle = (ms = 600) => new Promise((r) => setTimeout(r, ms));
          if (mode === 'live') {
            // in-page helper: direct WS RPC (used only for shape verification)
            const rpc = (method, params) =>
              win.webContents.executeJavaScript(`
                new Promise((res) => {
                  const ws = new WebSocket('ws://127.0.0.1:7474');
                  ws.onopen = () => ws.send(JSON.stringify({ id: 1, method: '${method}', params: ${JSON.stringify(params)} }));
                  ws.onmessage = (ev) => { ws.close(); res(JSON.parse(ev.data)); };
                  ws.onerror = () => res({ ok: false, error: 'ws error' });
                  setTimeout(() => res({ ok: false, error: 'ws timeout' }), 5000);
                })
              `);

            // in-page helper: drive an action through the real React components
            const ui = (js) => win.webContents.executeJavaScript(js);
            // wait until the React client is online (dot on) or timeout
            const waitOnline = async () => {
              for (let i = 0; i < 20; i++) {
                const d = await dom();
                if (/dot on/.test(d.dot)) return d;
                await settle(500);
              }
              throw new Error('client never reached online state');
            };
            const tapFind = (...needles) =>
              ui(
                `window.__wsTap.find((t) => t.dir === 'recv' && ${needles
                  .map((n) => `t.data.includes(${JSON.stringify(n)})`)
                  .join(' && ')}) ?? null`,
              );

            try {
              const dom1 = await waitOnline();
              console.log('SMOKE (live, initial):', JSON.stringify(dom1));
              const list = await rpc('list_devices', {});
              console.log('RPC list_devices:', JSON.stringify(list));
              if (!list.ok) throw new Error('list_devices not ok');
              if (!/dot on/.test(dom1.dot)) throw new Error('status dot not on');
              if (dom1.cards !== list.data.length) throw new Error(`cards ${dom1.cards} != devices ${list.data.length}`);
              if (dom1.offCards !== list.data.filter((d) => !d.available).length)
                throw new Error('unavailable card count mismatch');

              const target = process.env.LIVE_DEVICE
                ? list.data.find((d) => d.id === process.env.LIVE_DEVICE)
                : list.data.find((d) => d.available && d.zones.some((z) => z.matrix));
              if (!target) throw new Error('no available matrix device found');
              const zone = target.zones.find((z) => z.matrix);
              console.log('TARGET device:', target.id, 'zone:', zone.name, zone.matrix);
              // card index in DOM order (list_devices order is preserved by the UI)
              const idx = list.data.findIndex((d) => d.id === target.id);

              const clearTap = () => ui('window.__wsTap.length = 0');
              await clearTap();

              // (d) set_color red — drive the card's color input like a user
              await ui(`
                (() => {
                  const cards = document.querySelectorAll('.card');
                  const input = cards[${idx}].querySelector('.color-btn input[type="color"]');
                  // React's valueTracker ignores plain .value writes — use the native setter
                  const setVal = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
                  setVal.call(input, '#ff0000');
                  input.dispatchEvent(new Event('input', { bubbles: true }));
                  input.dispatchEvent(new Event('pointerup', { bubbles: true })); // card listens on pointerup
                })()
              `);
              await settle();
              const sc = await tapFind(`"device":"${target.id}"`, '"color":[255,0,0]');
              console.log('UI set_color → daemon:', sc ? JSON.stringify(JSON.parse(sc.data)) : 'NOT FOUND');
              if (!sc || JSON.parse(sc.data).ok !== true) {
                const dump = await ui('JSON.stringify(window.__wsTap)');
                console.log('WS TAP DUMP:', dump);
                throw new Error('UI set_color red not ok');
              }
              await clearTap();

              // (e) wave via EffectStudio drawer
              await ui(`
                (() => {
                  const cards = document.querySelectorAll('.card');
                  [...cards[${idx}].querySelectorAll('button')].find((b) => b.textContent.trim() === 'Effect…').click();
                })()
              `);
              await settle(400);
              await ui(`
                (() => {
                  const tabs = [...document.querySelectorAll('.drawer .tab')];
                  tabs.find((t) => t.textContent.trim() === 'wave').click();
                })()
              `);
              await settle(400);
              await ui(`
                (() => {
                  const drawer = document.querySelector('.drawer');
                  [...drawer.querySelectorAll('button')].find((b) => b.textContent.trim() === 'Apply').click();
                })()
              `);
              await settle();
              const se = await tapFind(`"device":"${target.id}"`, '"effect":"wave"');
              console.log('UI set_effect wave → daemon:', se ? JSON.stringify(JSON.parse(se.data)) : 'NOT FOUND');
              if (!se || JSON.parse(se.data).ok !== true) throw new Error('UI wave not ok');
              await clearTap();

              // (f) stop via the card's Stop button
              await ui(`
                (() => {
                  const cards = document.querySelectorAll('.card');
                  [...cards[${idx}].querySelectorAll('button')].find((b) => b.textContent.trim() === 'Stop').click();
                })()
              `);
              await settle();
              const st = await tapFind(`"device":"${target.id}"`, '"stopped":true');
              console.log('UI stop_effect → daemon:', st ? JSON.stringify(JSON.parse(st.data)) : 'NOT FOUND');
              if (!st || JSON.parse(st.data).ok !== true) throw new Error('UI stop not ok');
              await clearTap();

              // (g) matrix: paint red top row via the editor
              await ui(`
                (() => {
                  const cards = document.querySelectorAll('.card');
                  [...cards[${idx}].querySelectorAll('button')].find((b) => b.textContent.trim() === 'Edit matrix').click();
                })()
              `);
              await settle(500);
              await ui(`
                (() => {
                  const editor = document.querySelector('.drawer.matrix');
                  const keys = [...editor.querySelectorAll('.key')];
                  const brush = editor.querySelector('.tools input[type="color"]');
                  brush.value = '#ff0000';
                  brush.dispatchEvent(new Event('input', { bubbles: true }));
                  brush.dispatchEvent(new Event('change', { bubbles: true }));
                  // paint the first row (top row = first ${zone.matrix[1]} keys)
                  for (let i = 0; i < ${zone.matrix[1]}; i++) {
                    const k = keys[i];
                    k.dispatchEvent(new Event('pointerdown', { bubbles: true }));
                    k.dispatchEvent(new Event('pointerenter', { bubbles: true }));
                    k.dispatchEvent(new Event('pointerup', { bubbles: true }));
                  }
                })()
              `);
              await settle(400);
              await ui(`
                (() => {
                  const editor = document.querySelector('.drawer.matrix');
                  [...editor.querySelectorAll('button')].find((b) => b.textContent.trim() === 'Apply').click();
                })()
              `);
              await settle();
              const sm = await tapFind(`"device":"${target.id}"`, `"zone":"${zone.name}"`);
              console.log('UI set_matrix → daemon:', sm ? JSON.stringify(JSON.parse(sm.data)) : 'NOT FOUND');
              if (!sm || JSON.parse(sm.data).ok !== true || JSON.parse(sm.data).data.leds !== zone.leds)
                throw new Error('UI set_matrix not ok');

              // final: cards still healthy
              const dom2 = await dom();
              console.log('SMOKE (live, final):', JSON.stringify(dom2));
              if (dom2.cards !== list.data.length) throw new Error('cards lost after actions');
              if (errors.length) throw new Error(`page errors: ${errors.join(' | ')}`);
              console.log('PAGE ERRORS: none');
              console.log('SMOKE PASS (live)');
              if (process.env.SMOKE_HOLD_WAVE) {
                // perf-sampling mode: leave a wave running, driven through the UI
                await ui(`
                  (() => {
                    const cards = document.querySelectorAll('.card');
                    [...cards[${idx}].querySelectorAll('button')].find((b) => b.textContent.trim() === 'Effect…').click();
                  })()
                `);
                await settle(400);
                await ui(`[...document.querySelectorAll('.drawer .tab')].find((t) => t.textContent.trim() === 'wave').click()`);
                await settle(400);
                await ui(`
                  (() => {
                    const drawer = document.querySelector('.drawer');
                    [...drawer.querySelectorAll('button')].find((b) => b.textContent.trim() === 'Apply').click();
                  })()
                `);
                await settle(800);
                console.log('HOLD: wave running via UI — sampling window open');
              }
              await finish(true);
            } catch (e) {
              console.log('SMOKE FAIL (live):', e.message);
              console.log('PAGE ERRORS:', errors.length ? errors : 'none');
              app.exit(1);
            }
            return;
          }

          const expectOffline = mode === 'offline';
          // Studio flow: sidebar renders, Studio tab opens, preset adds a
          // layer, layer list changes, canvas exists with rows×cols cells.
          const studioFlow = async () => {
            const sidebar = await win.webContents.executeJavaScript(
              `JSON.stringify({ items: [...document.querySelectorAll('.nav-item')].map((b) => b.textContent.trim()) })`,
            ).then(JSON.parse);
            console.log('SIDEBAR:', JSON.stringify(sidebar));
            if (sidebar.items.length !== 3 || !sidebar.items.join(' ').includes('Studio'))
              throw new Error('sidebar nav missing');

            await win.webContents.executeJavaScript(
              `document.querySelectorAll('.nav-item')[1].click()`,
            );
            await settle(600);
            const s1 = await win.webContents.executeJavaScript(
              `JSON.stringify({
                 canvas: !!document.querySelector('.live-canvas'),
                 layerRows: document.querySelectorAll('.layer-row').length,
                 presets: [...document.querySelectorAll('.preset')].map((b) => b.textContent.trim()),
               })`,
            ).then(JSON.parse);
            console.log('STUDIO (initial):', JSON.stringify(s1));
            if (!s1.canvas) throw new Error('live canvas missing');
            if (s1.presets.length !== 6) throw new Error(`expected 6 presets, got ${s1.presets.length}`);

            // preset click → stack length changes (1 layer), canvas still alive
            await win.webContents.executeJavaScript(
              `[...document.querySelectorAll('.preset')].find((b) => b.textContent.trim() === 'Fire').click()`,
            );
            await settle(400);
            const s2 = await win.webContents.executeJavaScript(
              `JSON.stringify({
                 layerRows: document.querySelectorAll('.layer-row').length,
                 firstLayer: document.querySelector('.layer-row .layer-name')?.textContent ?? 'none',
                 canvas: !!document.querySelector('.live-canvas'),
               })`,
            ).then(JSON.parse);
            console.log('STUDIO (after Fire preset):', JSON.stringify(s2));
            if (s2.layerRows !== 1) throw new Error(`preset click: expected 1 layer, got ${s2.layerRows}`);
            if (s2.firstLayer !== 'fire') throw new Error(`preset layer is ${s2.firstLayer}, expected fire`);
            if (!s2.canvas) throw new Error('canvas vanished');

            // Apply via the bottom bar (explicit, not apply-live)
            await win.webContents.executeJavaScript(
              `[...document.querySelectorAll('.studio-bottom button')].find((b) => b.textContent.trim() === 'Apply').click()`,
            );
            await settle(600);
            const applied = await win.webContents.executeJavaScript(
              `document.querySelector('.apply-state')?.textContent.trim() ?? 'none'`,
            );
            console.log('STUDIO apply state:', applied);
            if (applied !== 'applied ✓') throw new Error(`apply state is "${applied}"`);

            // back to Devices: cards still intact
            await win.webContents.executeJavaScript(`document.querySelectorAll('.nav-item')[0].click()`);
            await settle(400);
          };

          win.webContents
            .executeJavaScript(
              `JSON.stringify({
                 dot: document.querySelector('.dot')?.className ?? 'none',
                 cards: document.querySelectorAll('.card').length,
                 banner: document.querySelector('.banner')?.textContent?.trim() ?? 'none',
                 buttons: document.querySelectorAll('.card-actions button').length,
                 sidebar: document.querySelectorAll('.nav-item').length,
               })`,
            )
            .then(async (json) => {
              const r = JSON.parse(json);
              let pass = expectOffline
                ? /dot off/.test(r.dot) && /offline/i.test(r.banner) && errors.length === 0
                : /dot on/.test(r.dot) && r.cards === 3 && errors.length === 0;
              let studioErr = null;
              if (pass && !expectOffline) {
                try {
                  await studioFlow();
                } catch (e) {
                  studioErr = e.message;
                  pass = false;
                }
              }
              console.log('SMOKE:', JSON.stringify(r));
              console.log('PAGE ERRORS:', errors.length ? errors : 'none');
              if (studioErr) console.log('STUDIO FAIL:', studioErr);
              console.log(pass ? 'SMOKE PASS' : 'SMOKE FAIL');
              finish(pass);
            })
            .catch((e) => {
              console.log('SMOKE FAIL (executeJavaScript):', e);
              app.exit(1);
            });
        }, 3000); // mock connect (250ms) + get_state round trip + paint; live: daemon connect
      });

  win.loadFile(`${dir}/index.html`).catch((e) => {
    console.log('SMOKE FAIL (load):', e);
    app.exit(1);
  });
});
