#!/usr/bin/env node
// End-to-end stdio JSON-RPC protocol verification for the compiled Makefile
// build provider backend:
//   - Negotiates litheLogging 0 and litheBuild 0
//   - Parses a realistic Go-style Makefile (with include, .PHONY, ## docs)
//   - Verifies lithe/build/model, lithe/build/prepareRun, lithe/build/execute
//   - Verifies failure path (stale target) and structured lithe/log events
//   - Cleanly shuts down the backend process with zero leftover processes

import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";

const backendPath = resolve(".artifacts/backend-darwin/backend");
const workspace = mkdtempSync(join(tmpdir(), "lithe-makefile-e2e-"));

try {
  mkdirSync(join(workspace, "mk"), { recursive: true });
  writeFileSync(
    join(workspace, "mk/extra.mk"),
    ".PHONY: lint\nlint: ## Run Go linters\n\t@echo lint-ok\n",
  );
  writeFileSync(
    join(workspace, "Makefile"),
    [
      "include mk/extra.mk",
      ".PHONY: run dev build test _internal",
      "",
      "build: ## Build Go binary",
      "\t@echo built-ok",
      "",
      "run: ## Run Go service",
      "\t@echo running-service",
      "",
      "dev: ## Start local dev server",
      "\t@echo dev-started",
      "",
      "test:",
      "\t@echo test-ok",
      "",
      "_internal:",
      "\t@echo helper",
      "",
    ].join("\n"),
  );

  const proc = spawn(backendPath, [], {
    cwd: workspace,
    stdio: ["pipe", "pipe", "pipe"],
  });

  let stderr = "";
  proc.stderr.on("data", (chunk) => {
    stderr += chunk.toString("utf8");
  });

  let buffer = Buffer.alloc(0);
  const pending = new Map();
  const notifications = [];

  proc.stdout.on("data", (chunk) => {
    buffer = Buffer.concat([buffer, chunk]);
    while (true) {
      const headerEnd = buffer.indexOf("\r\n\r\n");
      if (headerEnd === -1) break;
      const header = buffer.subarray(0, headerEnd).toString("ascii");
      const match = /Content-Length:\s*(\d+)/i.exec(header);
      if (!match) throw new Error(`Missing Content-Length header: ${header}`);
      const length = Number(match[1]);
      const bodyStart = headerEnd + 4;
      if (buffer.length < bodyStart + length) break;
      const body = buffer.subarray(bodyStart, bodyStart + length).toString("utf8");
      buffer = buffer.subarray(bodyStart + length);
      const msg = JSON.parse(body);
      if (msg.id !== undefined && pending.has(msg.id)) {
        const { resolve: res } = pending.get(msg.id);
        pending.delete(msg.id);
        res(msg);
      } else {
        notifications.push(msg);
      }
    }
  });

  let nextId = 1;
  function send(msg) {
    const json = JSON.stringify(msg);
    const frame = `Content-Length: ${Buffer.byteLength(json, "utf8")}\r\n\r\n${json}`;
    proc.stdin.write(frame);
  }
  function request(method, params) {
    const id = nextId++;
    return new Promise((res, rej) => {
      const timer = setTimeout(() => rej(new Error(`Timeout waiting for ${method}`)), 10_000);
      pending.set(id, {
        resolve: (msg) => {
          clearTimeout(timer);
          res(msg);
        },
      });
      send({ jsonrpc: "2.0", id, method, params });
    });
  }

  // 1. Initialize & negotiate litheLogging 0 + litheBuild 0
  const init = await request("initialize", {
    processId: process.pid,
    rootUri: pathToFileURL(workspace).href,
    capabilities: {
      experimental: {
        litheLogging: { versions: [0] },
        litheBuild: { versions: [0] },
      },
    },
  });
  assert.equal(init.error, undefined, `initialize failed: ${JSON.stringify(init.error)}`);
  assert.equal(init.result.capabilities.experimental.litheLogging.version, 0);
  assert.equal(init.result.capabilities.experimental.litheBuild.version, 0);
  send({ jsonrpc: "2.0", method: "initialized", params: {} });

  // 2. lithe/build/model
  const modelResp = await request("lithe/build/model", {
    buildSystemID: "makefile",
    root: ".",
    options: {},
    optionArguments: [],
    environment: {},
  });
  assert.equal(modelResp.error, undefined, `model failed: ${JSON.stringify(modelResp.error)}`);
  assert.equal(modelResp.result.format, "litheBuildModel");
  assert.equal(modelResp.result.version, 0);
  const targetNames = modelResp.result.targets.map((t) => t.name);
  assert.deepEqual(targetNames, ["build", "run", "dev", "test", "_internal", "lint"]);
  const devTarget = modelResp.result.targets.find((t) => t.name === "dev");
  assert.deepEqual(devTarget.tags, ["target"]);
  assert.equal(devTarget.properties.description, "Start local dev server");
  const buildTarget = modelResp.result.targets.find((t) => t.name === "build");
  assert.deepEqual(buildTarget.tags, ["target"]);
  const internalTarget = modelResp.result.targets.find((t) => t.name === "_internal");
  assert.deepEqual(internalTarget.tags, ["internal"]);

  // 3. lithe/build/prepareRun for "dev"
  const prepResp = await request("lithe/build/prepareRun", {
    operationID: "op-prep-1",
    buildSystemID: "makefile",
    root: ".",
    target: { name: "dev", path: "." },
    options: {},
    optionArguments: ["-j", "4"],
    environment: { ENV: "dev" },
    userArguments: ["PORT=8080"],
  });
  assert.equal(prepResp.error, undefined, `prepareRun failed: ${JSON.stringify(prepResp.error)}`);
  assert.deepEqual(prepResp.result.executable, { kind: "command", value: "make" });
  assert.deepEqual(prepResp.result.arguments, ["-j", "4", "dev", "PORT=8080"]);
  assert.equal(prepResp.result.workingDirectory, ".");
  assert.equal(prepResp.result.environment.ENV, "dev");

  // 4. lithe/build/prepareRun for stale target -> failure + lithe/log operationFailed
  const staleResp = await request("lithe/build/prepareRun", {
    operationID: "op-stale-1",
    buildSystemID: "makefile",
    root: ".",
    target: { name: "does-not-exist", path: "." },
    options: {},
    optionArguments: [],
    environment: {},
  });
  assert.ok(staleResp.error, "expected error for stale target");

  // 5. lithe/build/execute for real make target "build"
  const execResp = await request("lithe/build/execute", {
    operationID: "op-exec-1",
    buildSystemID: "makefile",
    root: ".",
    task: "run-target",
    target: { name: "build", path: "." },
    options: {},
    optionArguments: [],
    environment: {},
    userArguments: [],
  });
  assert.equal(execResp.error, undefined, `execute failed: ${JSON.stringify(execResp.error)}`);
  assert.equal(execResp.result.exitCode, 0);
  assert.match(execResp.result.output, /built-ok/);

  // 6. Verify structured lithe/log notifications
  const logEvents = notifications.filter((n) => n.method === "lithe/log").map((n) => n.params);
  assert.ok(
    logEvents.some((e) => e.kind === "operationFinished" && e.operation === "lithe/build/model"),
    `missing model operationFinished in ${JSON.stringify(logEvents)}`,
  );
  assert.ok(
    logEvents.some((e) => e.kind === "operationFailed" && e.operation === "lithe/build/prepareRun"),
    `missing prepareRun operationFailed in ${JSON.stringify(logEvents)}`,
  );
  assert.ok(
    logEvents.some((e) => e.kind === "operationFinished" && e.operation === "lithe/build/execute"),
    `missing execute operationFinished in ${JSON.stringify(logEvents)}`,
  );

  // 7. Shutdown cleanly
  const shutdownResp = await request("shutdown", null);
  assert.equal(shutdownResp.error, undefined);
  send({ jsonrpc: "2.0", method: "exit", params: null });

  const exitCode = await new Promise((res) => proc.on("close", res));
  assert.equal(exitCode, 0, `expected clean exit 0, got ${exitCode}, stderr: ${stderr}`);
  console.log("✓ Protocol E2E verified: initialize, model, prepareRun, stale-target error, execute, lithe/log, shutdown");
} finally {
  rmSync(workspace, { recursive: true, force: true });
}
