export function debounce(fn, ms) {
  let timer;
  return (...args) => {
    clearTimeout(timer);
    timer = setTimeout(() => fn(...args), ms);
  };
}

export function throttleRAF(fn) {
  let pending = false;
  return (...args) => {
    if (pending) return;
    pending = true;
    requestAnimationFrame(() => {
      fn(...args);
      pending = false;
    });
  };
}
