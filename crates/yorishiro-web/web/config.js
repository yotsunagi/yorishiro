// Edit this file (or bind-mount a replacement over it at deploy time, or point YSR_WEB_DIR /
// YORISHIRO_HOSTED_WEB_DIR at a directory with a replacement) if this dashboard needs to call
// a yorishiro-server that *isn't* the one serving these static files -- e.g. a
// yorishiro-hosted-server dashboard pointed at a separately deployed community server.
// Empty (the default) means same-origin, which is correct whenever the process serving this
// SPA is also the one serving the API -- true for both yorishiro-server and
// yorishiro-hosted-server out of the box, regardless of bind address/port.
window.YORISHIRO_CONFIG = {
  apiBase: "",
};
