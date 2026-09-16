// HOOK_VERSION=4
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
const out = process.env.GATE_TRACE_OUT;
if (out) {
  const root = process.env.GATE_TRACE_ROOT || process.cwd();
  const seen = new Set();
  const note = (p) => {
    try {
      if (p && typeof p === 'object' && typeof p.path === 'string') p = p.path;
      if (Buffer.isBuffer(p)) p = p.toString();
      if (typeof p !== 'string') return;
      if (p.startsWith('file:')) p = p.slice(5);
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
  const flush = () => {
    try { fs.writeFileSync(out, [...seen].sort().join('\n') + '\n'); } catch { /* the gate's verdict matters more than its trace */ }
  };
  process.on('exit', flush);
}
