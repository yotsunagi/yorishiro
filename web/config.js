// Edit this file (or bind-mount a replacement over it at deploy time) to point the
// dashboard at your yorishiro-server deployment. `/hosted/*` calls are always same-origin
// with this SPA, since yorishiro-hosted-server serves both the API and these static files;
// only `apiBase` (yorishiro-server's own origin) needs to be configured.
window.YORISHIRO_CONFIG = {
  apiBase: "http://localhost:8080",
};
