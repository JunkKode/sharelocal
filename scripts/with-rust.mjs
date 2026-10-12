#!/usr/bin/env node
/**
 * Run a command with the Rust toolchain reachable.
 *
 * `npm run` executes scripts through `/bin/sh`, which does not read `.zshenv`,
 * `.zshrc` or `.profile` — so a rustup install that works perfectly in an
 * interactive terminal is invisible here, and Tauri fails with a confusing
 * "failed to run 'cargo metadata'". This wrapper looks for cargo the way the
 * user's shell would, and puts it on PATH before spawning.
 *
 *   node scripts/with-rust.mjs tauri dev
 *   node scripts/with-rust.mjs cargo test --manifest-path src-tauri/Cargo.toml
 */

import { spawn, spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { homedir, platform } from "node:os";
import { delimiter, join } from "node:path";

const isWindows = platform() === "win32";
const exe = (name) => (isWindows ? `${name}.exe` : name);

/** Directories rustup and the common package managers install cargo into. */
function candidateBinDirs() {
  const dirs = [];
  if (process.env.CARGO_HOME) dirs.push(join(process.env.CARGO_HOME, "bin"));
  dirs.push(join(homedir(), ".cargo", "bin"));
  if (isWindows) {
    if (process.env.USERPROFILE) dirs.push(join(process.env.USERPROFILE, ".cargo", "bin"));
  } else {
    dirs.push("/usr/local/cargo/bin", "/opt/cargo/bin", "/opt/homebrew/bin", "/usr/local/bin");
  }
  return dirs;
}

function cargoOnPath() {
  const probe = spawnSync(exe("cargo"), ["--version"], { stdio: "ignore", shell: false });
  return probe.status === 0;
}

function ensureCargo() {
  if (cargoOnPath()) return true;

  for (const dir of candidateBinDirs()) {
    if (existsSync(join(dir, exe("cargo")))) {
      process.env.PATH = `${dir}${delimiter}${process.env.PATH ?? ""}`;
      return true;
    }
  }
  return false;
}

const [command, ...args] = process.argv.slice(2);
if (!command) {
  console.error("usage: node scripts/with-rust.mjs <command> [args…]");
  process.exit(2);
}

if (!ensureCargo()) {
  console.error(
    [
      "",
      "ShareLocal needs the Rust toolchain, and cargo could not be found.",
      "",
      "Install it with:",
      "  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh",
      "",
      "Then open a new terminal (or run `source \"$HOME/.cargo/env\"`) and try again.",
      "",
    ].join("\n"),
  );
  process.exit(1);
}

// Prefer this project's own binaries (tauri, vite) over anything global.
const localBin = join(process.cwd(), "node_modules", ".bin");
if (existsSync(localBin)) {
  process.env.PATH = `${localBin}${delimiter}${process.env.PATH}`;
}

const child = spawn(command, args, {
  stdio: "inherit",
  env: process.env,
  // Windows resolves `.cmd` shims only through a shell.
  shell: isWindows,
});

child.on("error", (err) => {
  console.error(`could not start ${command}: ${err.message}`);
  process.exit(1);
});
child.on("exit", (code, signal) => {
  if (signal) process.kill(process.pid, signal);
  else process.exit(code ?? 0);
});
