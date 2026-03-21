// RequestPilot — ISOLATED world content script
// Relay response body messages from the MAIN world to the background service worker.
window.addEventListener("message", (event) => {
  if (event.source !== window) return;
  if (event.data?.type !== "__REQUEST_PILOT_RESPONSE__") return;
  try {
    chrome.runtime.sendMessage({
      action: "captureResponseBody",
      url: event.data.url,
      body: event.data.body,
      status: event.data.status,
    }).catch(() => {}); // Silently handle disconnected port errors
  } catch {
    // Extension context may be invalidated after reload
  }
});
