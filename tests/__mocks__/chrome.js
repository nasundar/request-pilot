/**
 * In-memory mock for the chrome.* extension APIs used by RequestPilot.
 *
 * Storage is backed by a plain object so tests can inspect / reset state.
 * Every async API returns a Promise (matching Manifest V3 behaviour).
 */

const _store = {};

function resetStore() {
  for (const key of Object.keys(_store)) {
    delete _store[key];
  }
}

const chrome = {
  // Expose internals for test assertions / resets
  _store,
  _resetStore: resetStore,

  storage: {
    local: {
      get: jest.fn((keys) => {
        if (typeof keys === "string") {
          const result = {};
          if (keys in _store) result[keys] = _store[keys];
          return Promise.resolve(result);
        }
        if (Array.isArray(keys)) {
          const result = {};
          keys.forEach((k) => {
            if (k in _store) result[k] = _store[k];
          });
          return Promise.resolve(result);
        }
        // No keys – return full store copy
        return Promise.resolve({ ..._store });
      }),
      set: jest.fn((items) => {
        Object.assign(_store, items);
        return Promise.resolve();
      }),
    },
  },

  declarativeNetRequest: {
    getDynamicRules: jest.fn(() => Promise.resolve([])),
    updateDynamicRules: jest.fn(() => Promise.resolve()),
    onRuleMatchedDebug: {
      addListener: jest.fn(),
    },
  },

  runtime: {
    onInstalled: { addListener: jest.fn() },
    onStartup: { addListener: jest.fn() },
    onMessage: { addListener: jest.fn() },
    sendMessage: jest.fn(),
  },

  webRequest: {
    onBeforeRequest: { addListener: jest.fn() },
    onSendHeaders: { addListener: jest.fn() },
    onCompleted: { addListener: jest.fn() },
    onErrorOccurred: { addListener: jest.fn() },
  },
};

module.exports = chrome;
