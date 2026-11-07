// Smoke-test preload: taps window.WebSocket BEFORE app scripts run so the
// harness can assert the exact frames the React client exchanges with the
// daemon while real UI components (color input, studio, matrix editor) drive.
window.__wsTap = [];
const OrigWS = window.WebSocket;
window.WebSocket = class extends OrigWS {
  constructor(url, protocols) {
    super(url, protocols);
    this.addEventListener('message', (ev) => window.__wsTap.push({ dir: 'recv', data: String(ev.data) }));
    const orig = this.send.bind(this);
    this.send = (d) => {
      window.__wsTap.push({ dir: 'send', data: String(d) });
      orig(d);
    };
  }
};
