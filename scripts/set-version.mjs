#!/usr/bin/env node
/**
 * Set the app version everywhere it is written down, so a release tag is the
 * only thing that has to change.
 *
 *   node scripts/set-version.mjs 0.2.0
 *
 * Used by the release build (the tag `v0.2.0` becomes `0.2.0`); handy by hand
 * too. Touches tauri.conf.json (which names the installers), package.json and
 * src-tauri/Cargo.toml.
 */

import { readFileSync, writeFileSync } from "node:fs";

const version = process.argv[2];
// Semantic versions only: Windows installers reject anything else.
if (!version || !/^\d+\.\d+\.\d+$/.test(version)) {
  console.error(`usage: set-version.mjs <major.minor.patch>  (got "${version ?? ""}")`);
  process.exit(1);
}

function updateJson(path) {
  const data = JSON.parse(readFileSync(path, "utf8"));
  data.version = version;
  writeFileSync(path, `${JSON.stringify(data, null, 2)}\n`);
}

updateJson("src-tauri/tauri.conf.json");
updateJson("package.json");

// Only the [package] version — the first `version =` line in the file.
const cargoPath = "src-tauri/Cargo.toml";
const cargo = readFileSync(cargoPath, "utf8");
writeFileSync(cargoPath, cargo.replace(/^version = ".*"$/m, `version = "${version}"`));

console.log(`version set to ${version}`);
