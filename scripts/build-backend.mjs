#!/usr/bin/env node
// Builds the Rust build-provider backend into .artifacts/backend-<platform>/
// for scripts/package.mjs to package:
//
//   darwin          universal binary (aarch64 + x86_64) via lipo, ad-hoc signed
//   darwin-local    host-arch macOS binary for quick local iteration
//   windows-x64     x86_64-pc-windows-msvc  (backend.exe)
//   windows-arm64   aarch64-pc-windows-msvc (backend.exe)
//   linux-x64       x86_64-unknown-linux-musl (or gnu)
//   linux-arm64     aarch64-unknown-linux-musl (or gnu)
//
// Usage: node scripts/build-backend.mjs <darwin|darwin-local|windows-x64|windows-arm64|linux-x64|linux-arm64>

import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { copyFileSync, mkdirSync, readFileSync, rmSync } from "node:fs";
import { join, resolve } from "node:path";

const PLATFORMS = {
  darwin: {
    host: "darwin",
    targets: ["aarch64-apple-darwin", "x86_64-apple-darwin"],
    program: "backend",
  },
  "darwin-local": {
    host: "darwin",
    targets: [],
    program: "backend",
  },
  "windows-x64": {
    host: "win32",
    targets: ["x86_64-pc-windows-msvc"],
    program: "backend.exe",
  },
  "windows-arm64": {
    host: "win32",
    targets: ["aarch64-pc-windows-msvc"],
    program: "backend.exe",
  },
  "linux-x64": {
    host: "linux",
    targets: ["x86_64-unknown-linux-gnu"],
    program: "backend",
  },
  "linux-arm64": {
    host: "linux",
    targets: ["aarch64-unknown-linux-gnu"],
    program: "backend",
  },
};
const BACKEND = resolve("backend");
const CARGO_TIMEOUT_MS = 20 * 60_000;

const platform = process.argv[2] ?? "darwin";
const spec = PLATFORMS[platform];
if (!spec) {
  console.error(`usage: node scripts/build-backend.mjs <${Object.keys(PLATFORMS).join("|")}>`);
  process.exit(2);
}
if (process.platform !== spec.host) {
  console.error(`✗ ${platform} builds on ${spec.host}, this is ${process.platform}`);
  process.exit(1);
}

const outName = platform === "darwin-local" ? "darwin" : platform;
const output = resolve(".artifacts", `backend-${outName}`);
rmSync(output, { recursive: true, force: true });
mkdirSync(output, { recursive: true });

const program = join(output, spec.program);
if (spec.targets.length === 0) {
  execFileSync("cargo", ["build", "--release", "--locked"], {
    cwd: BACKEND, stdio: "inherit", timeout: CARGO_TIMEOUT_MS,
  });
  copyFileSync(join(BACKEND, "target", "release", spec.program), program);
} else {
  const built = spec.targets.map((target) => {
    execFileSync("cargo", ["build", "--release", "--locked", "--target", target], {
      cwd: BACKEND, stdio: "inherit", timeout: CARGO_TIMEOUT_MS,
    });
    return join(BACKEND, "target", target, "release", spec.program);
  });
  if (built.length === 1) {
    copyFileSync(built[0], program);
  } else {
    execFileSync("lipo", ["-create", ...built, "-output", program], { stdio: "inherit", timeout: 60_000 });
    execFileSync("codesign", ["--sign", "-", "--force", "--timestamp=none", program], { stdio: "inherit", timeout: 60_000 });
    execFileSync("codesign", ["--verify", "--strict", program], { stdio: "inherit", timeout: 60_000 });
  }
}

const sha256 = createHash("sha256").update(readFileSync(program)).digest("hex");
console.log(`.artifacts/backend-${outName}/${spec.program} ${sha256}`);
