// Static content of the theme preview's mock window. Plain data only: the
// build script (scripts/build-theme-preview.mjs) imports MOCK_FILE_NAMES from
// here to regenerate file-icons.json.

/** Explorer rows: [depth, name, kind, git status, selected]. */
export const MOCK_TREE = [
  [0, 'src', 'open'],
  [1, 'main.rs', 'file'],
  [1, 'server.rs', 'file', 'M'],
  [0, 'web', 'open'],
  [1, 'app.tsx', 'file', null, true],
  [1, 'styles.css', 'file', '?'],
  [0, 'docs', 'closed'],
  [0, 'Cargo.toml', 'file'],
  [0, 'package.json', 'file'],
  [0, 'README.md', 'file'],
];

export const MOCK_TABS = ['app.tsx', 'server.rs', 'README.md'];

/** Every file name that needs an icon. */
export const MOCK_FILE_NAMES = [...new Set([...MOCK_TREE.filter((r) => r[2] === 'file').map((r) => r[1]), ...MOCK_TABS])];

// Token kinds follow what Monaco colors in the real editor: k keyword,
// s string, t type (any capitalized identifier), n number, c comment.
export const MOCK_CODE = [
  [['c', '// Greets whoever opens the app.']],
  [['k', 'import'], ['', ' { useState } '], ['k', 'from'], ['', ' '], ['s', '"react"'], ['', ';']],
  [['k', 'import'], ['', ' '], ['k', 'type'], ['', ' { '], ['t', 'User'], ['', ' } '], ['k', 'from'], ['', ' '], ['s', '"./types"'], ['', ';']],
  [],
  [['k', 'const'], ['', ' '], ['t', 'MAX_NAMES'], ['', ' = '], ['n', '12'], ['', ';']],
  [],
  [['k', 'export'], ['', ' '], ['k', 'function'], ['', ' '], ['t', 'Greeting'], ['', '({ user }: { user: '], ['t', 'User'], ['', ' }) {']],
  [['', '  '], ['k', 'const'], ['', ' [count, setCount] = useState('], ['n', '0'], ['', ');']],
  [['', '  '], ['k', 'const'], ['', ' label = count > '], ['t', 'MAX_NAMES'], ['', ' ? '], ['s', '"everyone"'], ['', ' : user.name;']],
  [],
  [['', '  '], ['k', 'return'], ['', ' (']],
  [['', '    <button onClick={() => setCount(count + '], ['n', '1'], ['', ')}>']],
  [['', '      '], ['t', 'Hello'], ['', ', {label}!']],
  [['', '    </button>']],
  [['', '  );']],
  [['', '}']],
];
export const MOCK_ACTIVE_LINE = 9;

/** Terminal lines: [terminal color key, text, bold]. */
const PROMPT = [['green', '➜ ', true], ['cyan', ' moonlight ', true], ['blue', 'git:(', true], ['red', 'main', true], ['blue', ') ', true], ['yellow', '✗ ']];
export const MOCK_TERMINAL = [
  [...PROMPT, ['foreground', 'cargo run']],
  [['green', '   Compiling', true], ['foreground', ' moonlight v0.4.2']],
  [['yellow', 'warning', true], ['foreground', ': unused variable: `port`']],
  [['green', '    Finished', true], ['foreground', ' dev [unoptimized] target(s) in 2.41s']],
  [['green', '     Running', true], ['foreground', ' `target/debug/moonlight`']],
  [['foreground', 'listening on '], ['brightBlue', 'http://127.0.0.1:4000']],
  [['red', 'error', true], ['foreground', ': address already in use']],
  [...PROMPT],
];
