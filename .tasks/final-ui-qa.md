# Final QA + UI reconciliation phase — ChromaRGB

Plan: /Users/zari/.cursor/plans/chromargb_universal_rgb_control_684920b1.plan.md (read it first).
Everything is built: Rust daemon (43 tests green, release binary already at app/resources/chromargb-daemon,
real-hardware WebSocket E2E passed — BlackWidow Elite lights up), Electron+React UI (mock-verified).
One known mismatch is yours to fix, then you run the LIVE end-to-end.

Workspace: /Users/zari/Desktop/CHROMARGB (core/ + app/).

YOU OWN: app/src/** (all UI files), app/smoke.cjs, app/README.md.
Do NOT touch core/** — the daemon contract is FROZEN (it passed hardware E2E; adapt the UI, not the daemon).

## Real daemon contract (frozen, verified on the wire)

- ws://127.0.0.1:7474; requests {"id":N,"method":...,"params":{...}};
  responses {"id":N,"ok":true,"data":...} | {"id":N,"ok":false,"error":"..."}.
- list_devices -> data: ARRAY of DeviceInfo:
  {id, name, vendor, vid, pid, kind: "Razer"|"Logitech"|"Bridge"|"Generic",
   zones:[{name, leds, matrix:[rows,cols] (optional)}], available: bool, note (optional string)}.
  There is NO supportsPerLed field — derive it client-side as zones.some(z => z.matrix != null).
  Unavailable devices (G502 without permission): still listed, available:false, note may explain.
- get_state -> data: ARRAY of {id: string, color: [r,g,b] or null, effect: string or null}.
  (NOT {devices, active} as the UI currently assumes.)
- set_color {device, color:[r,g,b]} -> {device, color}.
- set_effect {device, effect, params} -> {device, effect}.
  Effects: static{color}, wave{speed, direction}, spectrum{speed}, breathe{speed, color?}, gradient{colors}.
- stop_effect {device} -> {device, stopped:true}.
- set_matrix {device, zone, colors:[[r,g,b],...]} -> {device, zone, leds}. Row-major within zone matrix.

## Tasks

1. Update app/src/types.ts (+ mock.ts, api.ts, anywhere they ripple) to the frozen contract:
   DeviceInfo with kind/available/note, derived supportsPerLed, StateEntry array for get_state.
   Update App.tsx + components to consume the new shapes: per-device status dot from available
   (gray-out unavailable cards with the note as tooltip/subtitle), effect state from get_state
   array (merge by id). Update mock.ts to emit the SAME shapes (mock devices: one available
   Razer-style matrix keyboard, one available:false Logitech-style mouse with note, one bridge device).
2. Check app/electron/main.ts daemon spawn path still matches app/resources/chromargb-daemon
   (it should — binary already placed). Only fix if broken.
3. npm run build must pass clean (tsc + vite). npm run check (selfcheck) must pass — update
   selfcheck assertions to the new shapes.
4. LIVE E2E (the payoff — real daemon, real keyboard):
   - Start the daemon in the background (cd core && ./target/release/chromargb-daemon), confirm port 7474.
   - Run the app against it: either NO_ELECTRON=1 npm run dev + electron smoke.cjs pointed at the dev
     server (check smoke.cjs modes; adapt if needed), or full npm run dev (electron + vite).
   - VERIFY in the live UI: (a) status dot green, (b) device cards render from the REAL daemon
     (BlackWidow Elite + G502 HERO), (c) G502 card shows unavailable state gracefully,
     (d) set color red on the keyboard FROM THE UI — keyboard lights red,
     (e) effect studio: apply wave — keyboard animates, (f) stop via UI,
     (g) matrix editor: paint + apply — bands change.
   - Use electron smoke.cjs with DOM assertions (cards count, dot class) plus manual verification
     notes for the lighting: you cannot see the keyboard — state exactly which UI actions were driven
     and that each IPC response was ok:true. Keyboard lighting was already proven in the daemon E2E;
     your job is proving the UI drives the same calls successfully.
5. Capture a screenshot of the live UI (smoke.cjs SMOKE_SHOT support or electron capturePage)
   showing the real devices — save to app/e2e-screenshot.png and reference it.
6. Perf spot-check with UI open + wave running: ps the daemon AND electron helper processes
   (%cpu, rss) — report. UI target: renderer <2% CPU idle.
7. Update app/README.md: live-run section (daemon autospawn via app, permission note for Logitech
   on macOS: System Settings > Privacy & Security > Input Monitoring, or run daemon with sudo),
   mock mode unchanged.

## Rules

ponytail — minimal diffs, no refactors of working components. If the live UI reveals a daemon-side
bug, do NOT fix the daemon — report it verbatim instead.

## Return

Files changed; build/check outputs (tail); live E2E transcript (which UI actions drove which
ok:true responses — verbatim response JSON where feasible); screenshot path confirmation; perf
spot-check numbers; any daemon-side bugs found (expected: none).
