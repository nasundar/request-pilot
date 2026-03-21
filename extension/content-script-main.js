// RequestPilot — MAIN world content script
// Intercepts fetch() and XMLHttpRequest to capture response bodies.
// Posts captured data to the window so the ISOLATED world relay can forward it.

(function () {
  "use strict";
  const MAX_BODY_SIZE = 100000;

  const originalFetch = window.fetch;
  window.fetch = async function (...args) {
    let response;
    try {
      response = await originalFetch.apply(this, args);
    } catch (err) {
      throw err; // Don't interfere with original errors
    }
    // Capture in background — don't block the response
    try {
      const clone = response.clone();
      clone.text().then((text) => {
        const url = response.url || (typeof args[0] === "string" ? args[0] : args[0]?.url) || "";
        window.postMessage({
          type: "__REQUEST_PILOT_RESPONSE__",
          url,
          body: text.length > MAX_BODY_SIZE ? text.substring(0, MAX_BODY_SIZE) + "\n...[truncated]" : text,
          status: response.status,
        }, "*");
      }).catch(() => {});
    } catch {}
    return response;
  };

  const origOpen = XMLHttpRequest.prototype.open;
  const origSend = XMLHttpRequest.prototype.send;
  XMLHttpRequest.prototype.open = function (method, url, ...rest) {
    // Resolve relative URLs to absolute for matching with webRequest
    try {
      this._rpUrl = new URL(url, window.location.href).href;
    } catch {
      this._rpUrl = url;
    }
    return origOpen.call(this, method, url, ...rest);
  };
  XMLHttpRequest.prototype.send = function (...args) {
    this.addEventListener("load", function () {
      try {
        const text = this.responseText || "";
        window.postMessage({
          type: "__REQUEST_PILOT_RESPONSE__",
          url: this._rpUrl || "",
          body: text.length > MAX_BODY_SIZE ? text.substring(0, MAX_BODY_SIZE) + "\n...[truncated]" : text,
          status: this.status,
        }, "*");
      } catch {}
    });
    return origSend.apply(this, args);
  };
})();
