#!/usr/bin/env node

import { spawn } from "node:child_process";
import { resolve } from "node:path";
import { createInterface } from "node:readline";

const cwd = resolve(process.argv[2] || process.cwd());

function connect() {
  const child = spawn("codex", ["app-server", "--stdio"], {
    cwd,
    stdio: ["pipe", "pipe", "inherit"],
  });
  const pending = new Map();
  const turnWaiters = new Map();
  let nextId = 1;
  const lines = createInterface({ input: child.stdout });
  lines.on("line", (line) => {
    let message;
    try {
      message = JSON.parse(line);
    } catch {
      return;
    }
    if (message.method === "turn/completed") {
      const waiter = turnWaiters.get(message.params?.turn?.id);
      if (waiter) {
        turnWaiters.delete(message.params.turn.id);
        waiter(message.params.turn);
      }
      return;
    }
    if (message.id == null || !pending.has(message.id)) return;
    const request = pending.get(message.id);
    pending.delete(message.id);
    if (message.error) request.reject(new Error(`${request.method}: ${message.error.message}`));
    else request.resolve(message.result);
  });

  function request(method, params) {
    const id = nextId++;
    child.stdin.write(`${JSON.stringify({ method, id, params })}\n`);
    return new Promise((resolveRequest, rejectRequest) => {
      pending.set(id, { method, resolve: resolveRequest, reject: rejectRequest });
    });
  }

  async function initialize() {
    await request("initialize", {
      clientInfo: { name: "nekodex_p0", title: "Nekodex P0", version: "0.1.0" },
      capabilities: {
        experimentalApi: true,
        requestAttestation: false,
        optOutNotificationMethods: [],
      },
    });
    child.stdin.write(`${JSON.stringify({ method: "initialized", params: {} })}\n`);
  }

  async function close() {
    child.stdin.end();
    await new Promise((resolveExit, rejectExit) => {
      child.once("exit", (code) => {
        if (code === 0) resolveExit();
        else rejectExit(new Error(`app-server exited with code ${code}`));
      });
    });
  }

  function waitForTurn(turnId) {
    return new Promise((resolveTurn) => turnWaiters.set(turnId, resolveTurn));
  }

  function terminate() {
    child.kill("SIGTERM");
  }

  return { request, initialize, waitForTurn, close, terminate };
}

let threadId;
let activeConnection;
try {
  const creator = connect();
  activeConnection = creator;
  await creator.initialize();
  const started = await creator.request("thread/start", {
    cwd,
    approvalPolicy: "never",
    sandbox: "read-only",
    ephemeral: false,
    serviceName: "nekodex_p0_persistence_probe",
  });
  threadId = started.thread.id;
  const turn = await creator.request("turn/start", {
    threadId,
    input: [{
      type: "text",
      text: "Reply exactly P0_READY and do nothing else.",
      text_elements: [],
    }],
  });
  await creator.waitForTurn(turn.turn.id);
  await creator.close();

  const controller = connect();
  activeConnection = controller;
  await controller.initialize();
  const resumed = await controller.request("thread/resume", { threadId, excludeTurns: true });
  const read = await controller.request("thread/read", { threadId, includeTurns: false });
  await controller.request("thread/delete", { threadId });
  await controller.close();

  console.log(JSON.stringify({
    threadId,
    readAcrossProcess: read.thread.id === threadId,
    resumeAcrossProcess: resumed.thread.id === threadId,
    cleanup: "deleted",
    boundary: "idle persisted threads only; live desktop-owned turns remain unverified",
  }, null, 2));
} catch (error) {
  activeConnection?.terminate();
  console.error(error);
  console.error(threadId ? `probe thread may require cleanup: ${threadId}` : "no probe thread created");
  process.exitCode = 1;
}
