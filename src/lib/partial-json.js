// Parse JSON that is still being streamed.
//
// A tool call's input arrives as fragments of one JSON object. `JSON.parse`
// rejects everything but the final fragment, so nothing can be shown until the
// call is complete. This reads as much as is already certain: finished values,
// plus the text so far of a string value that is still being written.

/**
 * Close whatever `text` leaves open, or return `null` when no prefix of it is
 * usable. Numbers, literals and keys that may be incomplete are dropped; a
 * string value in progress is kept up to its last complete character.
 */
function closeOpenJson(text) {
  const stack = []; // '{' or '[' for each open container
  let inString = false;
  let escaped = false;
  let escapeAt = -1; // index of the backslash that began the latest escape
  let stringIsKey = false;
  let expectKey = false; // inside an object, where a key comes next
  let inScalar = false; // inside a number or literal

  // The longest prefix that becomes valid JSON by closing `safeDepth` containers.
  let safeEnd = -1;
  let safeDepth = 0;
  const markSafe = (end) => {
    safeEnd = end;
    safeDepth = stack.length;
  };

  for (let i = 0; i < text.length; i++) {
    const ch = text[i];
    if (inString) {
      if (escaped) escaped = false;
      else if (ch === '\\') {
        escaped = true;
        escapeAt = i;
      } else if (ch === '"') {
        inString = false;
        if (!stringIsKey) markSafe(i + 1);
      }
      continue;
    }
    if (ch === '"') {
      inString = true;
      stringIsKey = expectKey;
      expectKey = false;
      escapeAt = -1;
    } else if (ch === '{' || ch === '[') {
      stack.push(ch);
      expectKey = ch === '{';
      markSafe(i + 1);
    } else if (ch === '}' || ch === ']') {
      inScalar = false;
      stack.pop();
      expectKey = false;
      markSafe(i + 1);
    } else if (ch === ',') {
      if (inScalar) markSafe(i);
      inScalar = false;
      expectKey = stack[stack.length - 1] === '{';
    } else if (ch === ' ' || ch === '\n' || ch === '\r' || ch === '\t') {
      if (inScalar) markSafe(i);
      inScalar = false;
    } else if (ch !== ':') {
      inScalar = true;
    }
  }

  const closers = (depth) => {
    let out = '';
    for (let i = depth - 1; i >= 0; i--) out += stack[i] === '{' ? '}' : ']';
    return out;
  };

  if (inString && !stringIsKey) {
    // Drop an escape that has not finished arriving: a lone backslash or a
    // partial \uXXXX.
    let end = text.length;
    if (escapeAt >= 0 && (escaped || /^\\u[0-9a-fA-F]{0,3}$/.test(text.slice(escapeAt)))) {
      end = escapeAt;
    }
    return `${text.slice(0, end)}"${closers(stack.length)}`;
  }
  if (safeEnd < 0) return null;
  return text.slice(0, safeEnd) + closers(safeDepth);
}

/**
 * Parse `text` as JSON, tolerating a cut-off end. Returns `undefined` when
 * nothing usable has arrived yet or the text is not JSON at all.
 */
export function parsePartialJson(text) {
  if (!text) return undefined;
  try {
    return JSON.parse(text);
  } catch {
    // Incomplete: fall through and close what is open.
  }
  const closed = closeOpenJson(text);
  if (closed === null) return undefined;
  try {
    return JSON.parse(closed);
  } catch {
    return undefined;
  }
}
