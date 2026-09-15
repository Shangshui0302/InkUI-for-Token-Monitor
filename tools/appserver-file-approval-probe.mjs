#!/usr/bin/env node

import { spawn, spawnSync } from "node:child_process";
import {
  mkdtempSync,
  readFileSync,
  rmSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { isAbsolute, join, normalize } from "node:path";
import { createInterface } from "node:readline";

const markerName = "nekodex-file-approval-marker.txt";
const markerContent = "nekodex-file-approval-ok\n";
const timeoutMs = 120_000;
const repository = mkdtempSync(join(tmpdir(), "nekodex-file-approval-"));
const markerPath = join(repository, markerName);

let child;
let timeout;
let threadId;
let turnId;
let turn;
let nextId = 1;
let closing = false;
const pending = new Map();
const fileChangeItems = new Map();
const approvalRequests = [];
const resolvedRequestIds = new Set();
const completedItems = new Map();
const eventLog = [];

function runGit(args) {
  const result = spawnSync("git", args, { cwd: repository, encoding: "utf8" });
  if (result.status !== 0) {
    throw new Error(`git ${args.join(" ")} failed: ${result.stderr.trim()}`);
  }
  return result.stdout;
}

function send(message) {
  if (!child?.stdin.writable) throw new Error("app-server stdin is not writable");
  child.stdin.write(`${JSON.stringify(message)}\n`);
}

function request(method, params) {
  const id = nextId++;
  return new Promise((resolveRequest, rejectRequest) => {
    pending.set(id, { method, resolve: resolveRequest, reject: rejectRequest });
    send({ method, id, params });
  });
}

function pathMatchesMarker(path) {
  if (typeof path !== "string") return false;
  if (isAbsolute(path)) return normalize(path) === normalize(markerPath);
  return normalize(path) === markerName;
}

function changesOnlyTouchMarker(changes) {
  return Array.isArray(changes)
    && changes.length === 1
    && pathMatchesMarker(changes[0]?.path);
}

function recordFileChangeItem(message) {
  const item = message.params?.item;
  if (message.params?.threadId !== threadId || message.params?.turnId !== turnId) return;
  if (item?.type === "fileChange" && typeof item.id === "string") {
    fileChangeItems.set(item.id, item);
  }
}

function handleServerRequest(message) {
  const params = message.params || {};
  const scoped = params.threadId === threadId && params.turnId === turnId;

  if (message.method === "item/fileChange/requestApproval") {
    const item = fileChangeItems.get(params.itemId);
    const expected = scoped && changesOnlyTouchMarker(item?.changes);
    approvalRequests.push({
      requestId: message.id,
      method: message.method,
      itemId: params.itemId,
      expected,
    });
    send({ id: message.id, result: { decision: expected ? "accept" : "decline" } });
    return;
  }

  if (message.method === "item/commandExecution/requestApproval") {
    approvalRequests.push({
      requestId: message.id,
      method: message.method,
      itemId: params.itemId,
      expected: false,
    });
    send({ id: message.id, result: { decision: "decline" } });
    return;
  }

  if (message.method?.endsWith("requestApproval")) {
    approvalRequests.push({
      requestId: message.id,
      method: message.method,
      itemId: params.itemId,
      expected: false,
    });
    send({ id: message.id, result: { decision: "decline" } });
  }
}

function onMessage(message) {
  if (message.id != null && pending.has(message.id) && !message.method) {
    const entry = pending.get(message.id);
    pending.delete(message.id);
    if (message.error) entry.reject(new Error(`${entry.method}: ${message.error.message}`));
    else entry.resolve(message.result);
    return;
  }

  if (message.id != null && message.method) {
    handleServerRequest(message);
    return;
  }

  if (message.method === "item/started") recordFileChangeItem(message);
  if (message.method === "item/fileChange/patchUpdated") {
    const params = message.params || {};
    if (params.threadId === threadId && params.turnId === turnId) {
      const prior = fileChangeItems.get(params.itemId) || { id: params.itemId, type: "fileChange" };
      fileChangeItems.set(params.itemId, { ...prior, changes: params.changes });
    }
  }
  if (message.method === "serverRequest/resolved") {
    if (message.params?.threadId === threadId) resolvedRequestIds.add(message.params.requestId);
  }
  if (message.method === "item/completed") {
    recordFileChangeItem(message);
    if (message.params?.threadId === threadId && message.params?.turnId === turnId) {
      completedItems.set(message.params.item?.id, message.params.item);
    }
  }
  if (message.method === "turn/completed" && message.params?.turn?.id === turnId) {
    turn = message.params.turn;
  }
}

function waitForTurnCompletion() {
  return new Promise((resolveWait, rejectWait) => {
    const interval = setInterval(() => {
      if (!turn) return;
      clearInterval(interval);
      resolveWait(turn);
    }, 25);
    timeout = setTimeout(() => {
      clearInterval(interval);
      rejectWait(new Error(`probe timed out after ${timeoutMs} ms`));
    }, timeoutMs);
  });
}

async function stopServer() {
  if (!child || closing) return;
  closing = true;
  clearTimeout(timeout);
  if (child.stdin.writable) child.stdin.end();
  const exited = new Promise((resolveExit) => child.once("exit", resolveExit));
  await Promise.race([
    exited,
    new Promise((resolveWait) => setTimeout(resolveWait, 2_000)),
  ]);
  if (child.exitCode == null) {
    child.kill("SIGTERM");
    await Promise.race([
      exited,
      new Promise((resolveWait) => setTimeout(resolveWait, 2_000)),
    ]);
  }
  if (child.exitCode == null) {
    child.kill("SIGKILL");
    await exited;
  }
}

async function main() {
  runGit(["init", "--quiet"]);

  child = spawn("codex", ["app-server", "--stdio"], {
    cwd: repository,
    stdio: ["pipe", "pipe", "pipe"],
  });
  child.stderr.on("data", (chunk) => process.stderr.write(chunk));
  const lines = createInterface({ input: child.stdout });
  lines.on("line", (line) => {
    eventLog.push(line);
    try {
      onMessage(JSON.parse(line));
    } catch (error) {
      process.stderr.write(`event handler failed: ${error.message}\n`);
    }
  });

  await request("initialize", {
    clientInfo: {
      name: "nekodex_file_approval_probe",
      title: "Nekodex File Approval Probe",
      version: "0.1.0",
    },
    capabilities: {
      experimentalApi: true,
      requestAttestation: false,
      optOutNotificationMethods: [],
    },
  });
  send({ method: "initialized", params: {} });

  const started = await request("thread/start", {
    cwd: repository,
    approvalPolicy: "on-request",
    approvalsReviewer: "user",
    sandbox: "read-only",
    ephemeral: true,
    serviceName: "nekodex_file_approval_probe",
    developerInstructions: [
      "This is an isolated file-approval protocol probe.",
      `Use the apply_patch tool, and only that tool, to create ${markerName}.`,
      "Do not run shell commands and do not read, create, edit, or delete any other file.",
    ].join(" "),
  });
  threadId = started.thread.id;

  const startedTurn = await request("turn/start", {
    threadId,
    input: [{
      type: "text",
      text: `Use apply_patch to create exactly ${markerName} with exactly this UTF-8 content, including the final newline:\n${markerContent}Do nothing else.`,
      text_elements: [],
    }],
  });
  turnId = startedTurn.turn.id;
  await waitForTurnCompletion();

  const fileApprovals = approvalRequests.filter(
    ({ method }) => method === "item/fileChange/requestApproval",
  );
  const commandApprovals = approvalRequests.filter(
    ({ method }) => method === "item/commandExecution/requestApproval",
  );
  const accepted = fileApprovals.filter(({ expected }) => expected);
  const acceptedItem = accepted[0] && completedItems.get(accepted[0].itemId);
  const markerActual = readFileSync(markerPath, "utf8");
  const gitStatus = runGit(["status", "--short", "--untracked-files=all"])
    .trim()
    .split("\n")
    .filter(Boolean);

  const checks = {
    turnCompleted: turn.status === "completed",
    exactlyOneExpectedFileApproval: fileApprovals.length === 1 && accepted.length === 1,
    noCommandApprovalAccepted: commandApprovals.every(({ expected }) => !expected),
    approvalResolved: accepted.length === 1 && resolvedRequestIds.has(accepted[0].requestId),
    fileChangeCompleted: acceptedItem?.type === "fileChange" && acceptedItem.status === "completed",
    markerContentMatches: markerActual === markerContent,
    markerIsOnlyWorktreeChange: gitStatus.length === 1 && gitStatus[0] === `?? ${markerName}`,
  };
  const summary = {
    repository,
    threadId,
    turnId,
    ephemeral: started.thread.ephemeral,
    turnStatus: turn.status,
    fileApprovalRequests: fileApprovals.length,
    commandApprovalRequests: commandApprovals.length,
    acceptedFileApprovalItemId: accepted[0]?.itemId || null,
    checks,
  };
  console.log(JSON.stringify(summary, null, 2));
  if (Object.values(checks).some((value) => !value)) {
    throw new Error("file approval probe checks failed");
  }
}

try {
  await main();
} catch (error) {
  process.stderr.write(`${error.stack || error.message}\n`);
  const relevant = eventLog
    .map((line) => {
      try { return JSON.parse(line); } catch { return null; }
    })
    .filter((message) => message && (
      message.method?.includes("requestApproval")
      || message.method === "serverRequest/resolved"
      || message.method === "item/started"
      || message.method === "item/completed"
      || message.method === "turn/completed"
    ));
  process.stderr.write(`relevant events: ${JSON.stringify(relevant, null, 2)}\n`);
  process.exitCode = 1;
} finally {
  await stopServer();
  rmSync(repository, { recursive: true, force: true });
}
