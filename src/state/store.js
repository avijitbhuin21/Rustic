/**
 * Lightweight reactive store.
 * createStore({ key: value }) => { getState, setState, subscribe }
 */
export function createStore(initialState) {
  let state = { ...initialState };
  const listeners = {};

  function getState(key) {
    return key !== undefined ? state[key] : { ...state };
  }

  function setState(partial) {
    const changed = [];
    for (const key in partial) {
      if (state[key] !== partial[key]) {
        state[key] = partial[key];
        changed.push(key);
      }
    }
    for (const key of changed) {
      if (listeners[key]) {
        for (const cb of listeners[key]) {
          cb(state[key], key);
        }
      }
    }
  }

  function subscribe(key, callback) {
    if (!listeners[key]) listeners[key] = [];
    listeners[key].push(callback);
    return () => {
      listeners[key] = listeners[key].filter(cb => cb !== callback);
    };
  }

  return { getState, setState, subscribe };
}
