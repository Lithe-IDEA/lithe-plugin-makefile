#!/usr/bin/env node
// Packages io.github.lithe-idea.makefile-build into one archive per platform:
//
//   dist/makefile-build-<version>-darwin.zip         Apple Silicon and Intel Macs
//   dist/makefile-build-<version>-windows-x64.zip    Windows x64
//   dist/makefile-build-<version>-windows-arm64.zip  Windows ARM64
//   dist/makefile-build-<version>-linux-x64.zip      Linux x64
//   dist/makefile-build-<version>-linux-arm64.zip    Linux ARM64
//
// Usage: node scripts/package.mjs [--expect-tag <tag>] [--platforms darwin,windows-x64,windows-arm64,linux-x64,linux-arm64]

import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { chmodSync, cpSync, existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { parseArgs } from "node:util";

const PACKAGE_DIR = "io.github.lithe-idea.makefile-build";
const ASSET_PREFIX = "makefile-build";
const BACKEND_PATH = "bin/backend";
const PLATFORMS = {
  darwin: { program: "backend", builtOn: "macOS" },
  "windows-x64": { program: "backend.exe", builtOn: "Windows" },
  "windows-arm64": { program: "backend.exe", builtOn: "Windows" },
  "linux-x64": { program: "backend", builtOn: "Linux" },
  "linux-arm64": { program: "backend", builtOn: "Linux" },
};

const { values } = parseArgs({
  options: {
    "expect-tag": { type: "string" },
    platforms: { type: "string", default: Object.keys(PLATFORMS).join(",") },
  },
});
const platforms = values.platforms.split(",").filter(Boolean);

const source = readFileSync(`${PACKAGE_DIR}/plugin.json`, "utf8");
const manifest = JSON.parse(source);
const problems = [];
if (manifest.id !== PACKAGE_DIR) problems.push(`plugin.json id ${manifest.id} must equal ${PACKAGE_DIR}`);
if (!/^[0-9]+\.[0-9]+\.[0-9]+$/.test(manifest.version)) problems.push(`version ${manifest.version} is not MAJOR.MINOR.PATCH`);
if (manifest.vendor?.signatureRequirement !== "none") problems.push('vendor.signatureRequirement must be "none"');
if (manifest.id === "dev.lithe" || manifest.id.startsWith("dev.lithe.")) problems.push("dev.lithe.* is reserved for official plugins");
const cargoVersion = /^\[package\][^[]*?^version = "([^"]+)"/ms.exec(readFileSync("backend/Cargo.toml", "utf8"))?.[1];
if (cargoVersion !== manifest.version) problems.push(`backend/Cargo.toml version ${cargoVersion} must equal plugin.json version ${manifest.version}`);
if (values["expect-tag"] !== undefined && values["expect-tag"] !== `v${manifest.version}`) {
  problems.push(`tag ${values["expect-tag"]} does not match plugin.json version ${manifest.version} (expected v${manifest.version})`);
}
for (const platform of platforms) {
  const spec = PLATFORMS[platform];
  if (!spec) {
    problems.push(`unknown platform ${platform}; use ${Object.keys(PLATFORMS).join(", ")}`);
    continue;
  }
  if (!existsSync(join(".artifacts", `backend-${platform}`, spec.program))) {
    problems.push(`.artifacts/backend-${platform}/${spec.program} is missing; run node scripts/build-backend.mjs ${platform} on ${spec.builtOn}`);
  }
}
if (problems.length > 0) {
  for (const p of problems) console.error(`✗ ${p}`);
  process.exit(1);
}

rmSync("dist", { recursive: true, force: true });
mkdirSync("dist", { recursive: true });
const staging = resolve(".artifacts", "package");
for (const platform of platforms) {
  const { program } = PLATFORMS[platform];
  const built = join(".artifacts", `backend-${platform}`);
  rmSync(staging, { recursive: true, force: true });
  const root = join(staging, PACKAGE_DIR);
  cpSync(PACKAGE_DIR, root, { recursive: true, filter: (p) => !p.endsWith(".DS_Store") });
  mkdirSync(join(root, "bin"), { recursive: true });
  cpSync(join(built, program), join(root, "bin", program));
  chmodSync(join(root, "bin", program), 0o755);
  if (program !== "backend") {
    const forPlatform = JSON.parse(source);
    if (forPlatform.backend?.launch?.path === BACKEND_PATH) {
      forPlatform.backend.launch.path = `bin/${program}`;
    }
    for (const system of forPlatform.buildSystems ?? []) {
      if (system.provider?.launch?.path === BACKEND_PATH) {
        system.provider.launch.path = `bin/${program}`;
      }
    }
    writeFileSync(join(root, "plugin.json"), `${JSON.stringify(forPlatform, null, 2)}\n`);
  }
  const name = `${ASSET_PREFIX}-${manifest.version}-${platform}.zip`;
  execFileSync("zip", ["-qrXy", resolve("dist", name), PACKAGE_DIR], { cwd: staging, timeout: 60_000 });
  const sha256 = createHash("sha256").update(readFileSync(resolve("dist", name))).digest("hex");
  console.log(`dist/${name} ${sha256}`);
}
rmSync(staging, { recursive: true, force: true });
