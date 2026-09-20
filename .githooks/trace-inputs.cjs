// HOOK_VERSION=5
// Record every file and directory a node gate reads, so its input set is
// discovered instead of hand-maintained.
//
// Kenny chose this over a per-gate list of paths on 2026-09-16: a list a
// person keeps up to date goes stale the moment a gate opens one more
// file, and a stale list is exactly how a gate gets skipped on the commit
// where it would have found something. A recorded set cannot go stale,
// because it is rewritten by every green run.
//
// Loaded with NODE_OPTIONS=--require, so the gate itself needs no change.
// It patches the synchronous fs entry points plus the promise and callback
// forms; a path outside the repository, inside node_modules or inside .git
// is dropped, because none of those change between two commits of this
// repository and carrying them would only slow the hash down.
//
// A directory is recorded as well as its files: a gate that walks a
// directory notices a file being ADDED there, and that addition changes
// no file the gate previously read. gate-cache.sh hashes a recorded
// directory by its listing for exactly that reason.
'use strict';
const fs = require('fs');
const fsp = fs.promises;
const path = require('path');
const { fileURLToPath } = require('url');
const out = process.env.GATE_TRACE_OUT;
if (out) {
  const root = process.env.GATE_TRACE_ROOT || process.cwd();
  const seen = new Set();
  const note = (p) => {
    try {
      if (p instanceof URL) p = p.protocol === 'file:' ? fileURLToPath(p) : String(p);
      if (p && typeof p === 'object' && typeof p.path === 'string') p = p.path;
      if (Buffer.isBuffer(p)) p = p.toString();
      if (typeof p !== 'string') return;
      // A file: URL carries percent-escapes and, on some platforms, a host;
      // slicing the scheme off left `%20` in the path and resolved to a name
      // no gate had read, so the whole read was dropped [fix-57].
      if (p.startsWith('file:')) p = fileURLToPath(p);
      const abs = path.resolve(root, p);
      if (!abs.startsWith(root + path.sep)) return;
      const rel = path.relative(root, abs);
      if (rel.startsWith('node_modules' + path.sep) || rel.includes(path.sep + 'node_modules' + path.sep)) return;
      if (rel === '.git' || rel.startsWith('.git' + path.sep)) return;
      seen.add(rel);
    } catch { /* a path we cannot resolve is a path we cannot cache on */ }
  };
  const wrap = (obj, names) => {
    for (const fn of names) {
      const orig = obj[fn];
      if (typeof orig !== 'function') continue;
      obj[fn] = function (p, ...rest) { note(p); return orig.call(this, p, ...rest); };
    }
  };
  wrap(fs, ['readFileSync', 'readdirSync', 'statSync', 'lstatSync', 'existsSync',
            'openSync', 'realpathSync', 'accessSync', 'readFile', 'readdir',
            'stat', 'lstat', 'open', 'access']);
  wrap(fsp, ['readFile', 'readdir', 'stat', 'lstat', 'open', 'access', 'realpath']);
  // An ES module binds `import { readFileSync } from 'node:fs'` when the
  // module is evaluated, so it keeps the UNPATCHED function unless the
  // builtin's exports are re-synced after the patch. Without this a gate
  // written as an ES module recorded the modules it loaded and none of the
  // data it read: 14 of 33 kp-themes checks held two inputs or fewer [fix-57].
  try { require('module').syncBuiltinESMExports(); } catch { /* older node: the CommonJS patch still holds */ }
  const flush = () => {
    try { fs.writeFileSync(out, [...seen].sort().join('\n') + '\n'); } catch { /* the gate's verdict matters more than its trace */ }
  };
  process.on('exit', flush);
}
