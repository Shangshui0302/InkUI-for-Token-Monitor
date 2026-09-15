#!/usr/bin/env node

import { spawn } from "node:child_process";
import { createWriteStream } from "node:fs";
import { resolve } from "node:path";
import { createInterface } from "node:readline";

const cwd = resolve(process.argv[2] || process.cwd());
const logPath = resolve(process.argv[3] || "p0-appserver-events.jsonl");
const log = createWriteStream(logPath, { flags: "w", mode: 0o600 });
const child = spawn("codex", ["app-server", "--stdio"], {
  cwd,
  stdio: ["pipe", "pipe", "inherit"],
});
const lines = createInterface({ input: child.stdout });

let nextId = 1;
let threadId;
let turnId;
let approvalSeen = false;
let steerAccepted = false;
let interruptAccepted = false;
let finished = false;
let completedStatus;
const pending = new Map();

function send(message) {
  child.stdin.write(`${JSON.stringify(message)}\n`);
}

function request(method, params) {
  const id = nextId++;
  send({ method, id, params });
  return new Promise((resolveRequest, rejectRequest) => {
    pending.set(id, { method, resolve: resolveRequest, reject: rejectRequest });
  });
}

function maybeFinish() {
  if (!completedStatus || !interruptAccepted) return;
  finished = true;
  const summary = { threadId, turnId, approvalSeen, steerAccepted, interruptAccepted,
    status: completedStatus };
  console.log(JSON.stringify(summary, null, 2));
  child.stdin.end();
}

async function start() {
  await request("initialize", {
    clientInfo: { name: "nekodex_p0", title: "Nekodex P0", version: "0.1.0" },
    capabilities: {
      experimentalApi: true,
      requestAttestation: false,
      optOutNotificationMethods: [],
    },
  });
  send({ method: "initialized", params: {} });
  const thread = await request("thread/start", {
    cwd,
    approvalPolicy: "untrusted",
    approvalsReviewer: "user",
    sandbox: "workspace-write",
    ephemeral: true,
    serviceName: "nekodex_p0",
  });
  threadId = thread.thread.id;
  const turn = await request("turn/start", {
    threadId,
    input: [{
      type: "text",
      text: "Run exactly this shell command and do nothing else: printf p0-approval-ok > p0-approval.txt && sleep 30",
      text_elements: [],
    }],
  });
  turnId = turn.turn.id;
  const steer = await request("turn/steer", {
    threadId,
    expectedTurnId: turnId,
    input: [{ type: "text", text: "Keep the requested command unchanged.", text_elements: [] }],
  });
  steerAccepted = steer.turnId === turnId;
}

async function handleServerRequest(message) {
  const paramsText = JSON.stringify(message.params || {});
  const isExpected = message.params?.threadId === threadId
    && message.params?.turnId === turnId
    && paramsText.includes("p0-approval.txt");
  if (message.method === "item/commandExecution/requestApproval") {
    approvalSeen = isExpected;
    send({ id: message.id, result: { decision: isExpected ? "accept" : "decline" } });
    if (isExpected) {
      setTimeout(async () => {
        try {
          await request("turn/interrupt", { threadId, turnId });
          interruptAccepted = true;
          maybeFinish();
        } catch (error) {
          console.error(`interrupt failed: ${error.message}`);
        }
      }, 1200);
    }
    return;
  }
  if (message.method === "item/fileChange/requestApproval") {
    approvalSeen = isExpected;
    send({ id: message.id, result: { decision: isExpected ? "accept" : "decline" } });
    return;
  }
  if (message.method?.endsWith("requestApproval")) {
    send({ id: message.id, error: { code: -32001, message: "P0 probe declined unexpected approval" } });
  }
}

lines.on("line", (line) => {
  log.write(`${line}\n`);
  let message;
  try { message = JSON.parse(line); } catch { return; }
  if (message.id != null && pending.has(message.id)) {
    const entry = pending.get(message.id);
    pending.delete(message.id);
    if (message.error) entry.reject(new Error(`${entry.method}: ${message.error.message}`));
    else entry.resolve(message.result);
    return;
  }
  if (message.id != null && message.method) {
    handleServerRequest(message).catch((error) => console.error(error));
    return;
  }
  if (message.method === "turn/completed" && message.params?.turn?.id === turnId) {
    completedStatus = message.params.turn.status;
    maybeFinish();
  }
});

child.on("exit", (code) => {
  log.end();
  if (!finished || !approvalSeen || !steerAccepted || !interruptAccepted) process.exitCode = 1;
  else if (code !== 0 && code !== null) process.exitCode = code;
});

setTimeout(() => {
  if (!finished) {
    console.error("P0 lifecycle probe timed out");
    child.kill("SIGTERM");
    process.exitCode = 1;
  }
}, 90_000).unref();

start().catch((error) => {
  console.error(error);
  child.kill("SIGTERM");
  process.exitCode = 1;
});
