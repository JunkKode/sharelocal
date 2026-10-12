#!/usr/bin/env node
/**
 * Reclaim build-cache space.
 *
 * `src-tauri/target` is Rust's build cache, and it grows without bound: every
 * dependency is compiled per profile, and the incremental cache keeps a copy of
 * intermediate state for each. Ten gigabytes is normal after a few builds. None
 * of it is needed to *run* an installed ShareLocal.app — it only makes the next
 * build faster.
 *
 *   node scripts/clean.mjs           reclaim the dev cache (keeps release)
 *   node scripts/clean.mjs --all     reclaim everything, including release
 */

import { existsSync, rmSync, statSync, readdirSync } from "node:fs";
import { join } from "node:path";

const TARGET = join("src-tauri", "target");
const all = process.argv.includes("--all");

function sizeOf(path) {
  if (!existsSync(path)) return 0;
  let total = 0;
  const stack = [path];
  while (stack.length) {
    const current = stack.pop();
    let stats;
    try {
      stats = statSync(current);
    } catch {
      continue;
    }
    if (stats.isDirectory()) {
      try {
        for (const entry of readdirSync(current)) stack.push(join(current, entry));
      } catch {
        // Unreadable directory; skip it rather than abort the whole sweep.
      }
    } else {
      total += stats.size;
    }
  }
  return total;
}

const gb = (bytes) => `${(bytes / 1024 ** 3).toFixed(2)} GB`;

const targets = all
  ? [TARGET]
  : [join(TARGET, "debug"), join(TARGET, "tmp")];

let freed = 0;
for (const path of targets) {
  if (!existsSync(path)) continue;
  const size = sizeOf(path);
  rmSync(path, { recursive: true, force: true });
  freed += size;
  console.log(`removed ${path} (${gb(size)})`);
}

if (freed === 0) {
  console.log("nothing to clean.");
} else {
  console.log(`\nfreed ${gb(freed)}.`);
  console.log(
    all
      ? "The next build recompiles everything (a few minutes)."
      : "A packaged ShareLocal.app is unaffected; the next `npm run app` rebuilds the dev cache.",
  );
}
