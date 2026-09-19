#!/usr/bin/env python3
"""Small, read-only Codex status monitor.

The collector intentionally reads the files Codex already writes.  It never
starts an App Server, calls an online API, or mutates CODEX_HOME.
"""

from __future__ import annotations

import argparse
import json
import os
import sqlite3
import sys
import time
from datetime import datetime, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any, Iterable
from urllib.parse import parse_qs, urlparse


MAX_TAIL_BYTES = 3_000_000
MAX_ACTIVITY = 100
MAX_SESSIONS = 500
MAX_TEXT = 240
MAX_SCAN_ROLLOUTS = 32


def codex_home() -> Path:
    value = os.environ.get("CODEX_HOME")
    return Path(value).expanduser() if value else Path.home() / ".codex"


def workspace() -> Path | None:
    value = os.environ.get("CODEX_STATUS_WORKSPACE")
    return Path(value).expanduser().resolve() if value else None


def shorten(value: Any, limit: int = MAX_TEXT) -> str:
    if value is None:
        return ""
    text = " ".join(str(value).split())
    return text if len(text) <= limit else text[: limit - 1] + "…"


def epoch_from_value(value: Any) -> float | None:
    if value is None:
        return None
    if isinstance(value, (int, float)):
        number = float(value)
        return number / 1000 if number > 10_000_000_000 else number
    if isinstance(value, str):
        try:
            return datetime.fromisoformat(value.replace("Z", "+00:00")).timestamp()
        except ValueError:
            return None
    return None


def iso_time(epoch: float | None) -> str | None:
    return datetime.fromtimestamp(epoch, timezone.utc).isoformat().replace("+00:00", "Z") if epoch else None


def recent_lines(path: Path, max_bytes: int = MAX_TAIL_BYTES) -> Iterable[str]:
    try:
        with path.open("rb") as stream:
            stream.seek(0, os.SEEK_END)
            size = stream.tell()
            stream.seek(max(0, size - max_bytes))
            data = stream.read(max_bytes)
    except OSError:
        return ()
    text = data.decode("utf-8", errors="replace")
    if size > max_bytes:
        _, _, text = text.partition("\n")
    return text.splitlines()


def sqlite_connect(path: Path) -> sqlite3.Connection:
    return sqlite3.connect(f"file:{path}?mode=ro", uri=True, timeout=1)


def table_exists(connection: sqlite3.Connection, table: str) -> bool:
    return connection.execute(
        "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?", (table,)
    ).fetchone() is not None


def query_threads(current_workspace: Path | None = None) -> tuple[list[dict[str, Any]], list[str]]:
    path = codex_home() / "state_5.sqlite"
    if not path.exists():
        return [], ["sessions: state_5.sqlite 不存在"]
    try:
        with sqlite_connect(path) as connection:
            if not table_exists(connection, "threads"):
                return [], ["sessions: threads 表不存在"]
            rows = connection.execute(
                """
                SELECT id, rollout_path, cwd, title, name, preview, model,
                       reasoning_effort, tokens_used, updated_at, updated_at_ms,
                       archived, source, thread_source
                FROM threads
                WHERE archived = 0
                  AND COALESCE(thread_source, '') != 'subagent'
                ORDER BY COALESCE(updated_at_ms, updated_at * 1000) DESC
                LIMIT ?
                """,
                (MAX_SESSIONS,),
            ).fetchall()
    except (OSError, sqlite3.Error) as error:
        return [], [f"sessions: {error}"]

    result: list[dict[str, Any]] = []
    for row in rows:
        item = dict(zip((
            "id", "rollout_path", "cwd", "title", "name", "preview", "model",
            "reasoning_effort", "tokens_used", "updated_at", "updated_at_ms",
            "archived", "source", "thread_source",
        ), row))
        if current_workspace and item.get("cwd") != str(current_workspace):
            continue
        updated = epoch_from_value(item.get("updated_at_ms") or item.get("updated_at"))
        result.append({
            "id": item["id"],
            "title": shorten(item.get("name") or item.get("title"), 120),
            "preview": shorten(item.get("preview"), 240),
            "workspace": item.get("cwd"),
            "model": item.get("model"),
            "reasoning_effort": item.get("reasoning_effort"),
            "tokens_used": item.get("tokens_used") or 0,
            "updated_at": iso_time(updated),
            "rollout_path": item.get("rollout_path"),
        })
    return result, []


def query_goal(thread_id: str | None) -> dict[str, Any] | None:
    if not thread_id:
        return None
    path = codex_home() / "goals_1.sqlite"
    if not path.exists():
        return None
    try:
        with sqlite_connect(path) as connection:
            if not table_exists(connection, "thread_goals"):
                return None
            row = connection.execute(
                "SELECT objective, status, token_budget, tokens_used, time_used_seconds, updated_at_ms "
                "FROM thread_goals WHERE thread_id=?",
                (thread_id,),
            ).fetchone()
    except sqlite3.Error:
        return None
    if not row:
        return None
    return dict(zip(("objective", "status", "token_budget", "tokens_used", "time_used_seconds", "updated_at_ms"), row))


def extract_text(value: Any) -> str:
    if isinstance(value, str):
        return shorten(value)
    if isinstance(value, list):
        return shorten(" ".join(extract_text(item) for item in value))
    if isinstance(value, dict):
        for key in ("text", "content", "summary_text", "message", "description", "command"):
            if key in value:
                text = extract_text(value[key])
                if text:
                    return text
    return ""


def activity_from_line(line: dict[str, Any], fallback_epoch: float) -> dict[str, Any] | None:
    timestamp = epoch_from_value(line.get("timestamp")) or fallback_epoch
    payload = line.get("payload") or {}
    event_type = payload.get("type") if isinstance(payload, dict) else None
    item = payload.get("item") if isinstance(payload, dict) else None
    if line.get("type") == "response_item" and isinstance(payload, dict):
        event_type = payload.get("type") or event_type
        item = payload
    if event_type not in {"task_started", "user_message", "thread_started", "turn_started", "item_started", "item_completed", "turn_completed", "error", "warning", "message"}:
        return None
    message = extract_text(item or payload)
    return {
        "kind": event_type,
        "message": message,
        "occurred_at": iso_time(timestamp),
    }


def parse_rollout(path: str | None) -> dict[str, Any]:
    result: dict[str, Any] = {"activity": [], "rate_limits": None, "last_user": None, "last_final": None, "last_event": None, "error": None}
    if not path:
        result["error"] = "rollout 路径为空"
        return result
    rollout = Path(path).expanduser()
    if not rollout.is_file():
        result["error"] = "rollout 文件不存在"
        return result
    activities: list[dict[str, Any]] = []
    try:
        for raw in recent_lines(rollout):
            try:
                line = json.loads(raw)
            except json.JSONDecodeError:
                continue
            timestamp = epoch_from_value(line.get("timestamp")) or time.time()
            result["last_event"] = max(result["last_event"] or 0, timestamp)
            payload = line.get("payload") or {}
            if line.get("type") == "event_msg" and isinstance(payload, dict):
                item = payload.get("item") or {}
                item_type = item.get("type") if isinstance(item, dict) else None
                if payload.get("type") in {"user_message", "task_started"} or item_type in {"UserMessage", "userMessage"}:
                    result["last_user"] = max(result["last_user"] or 0, timestamp)
                if payload.get("type") in {"turn_completed", "task_completed"} or item_type in {"AgentMessage", "agentMessage"}:
                    result["last_final"] = max(result["last_final"] or 0, timestamp)
            if isinstance(payload, dict) and payload.get("type") == "token_count":
                info = payload.get("info") or {}
                if isinstance(info, dict) and payload.get("rate_limits"):
                    result["rate_limits"] = payload["rate_limits"]
            activity = activity_from_line(line, timestamp)
            if activity:
                activities.append(activity)
    except OSError as error:
        result["error"] = str(error)
    result["activity"] = activities[-MAX_ACTIVITY:]
    return result


def normalize_quota(rate_limits: dict[str, Any] | None) -> dict[str, Any] | None:
    if not isinstance(rate_limits, dict):
        return None
    windows = []
    for key, label in (("primary", "5 小时"), ("secondary", "周")):
        window = rate_limits.get(key)
        if not isinstance(window, dict):
            continue
        used = window.get("used_percent", window.get("usedPercent"))
        try:
            used = float(used)
        except (TypeError, ValueError):
            continue
        if not 0 <= used <= 100:
            continue
        reset = window.get("resets_at", window.get("resetsAt"))
        windows.append({"name": label, "remaining_percent": round(100 - used, 2), "resets_at": iso_time(epoch_from_value(reset))})
    if not windows:
        return None
    return {"account_id": rate_limits.get("limit_id") or "codex", "windows": windows}


def find_recent_threads(current_workspace: Path | None) -> list[dict[str, Any]]:
    threads, _ = query_threads(current_workspace)
    return threads[:MAX_SCAN_ROLLOUTS]


def build_status() -> dict[str, Any]:
    now = time.time()
    current_workspace = workspace()
    sessions, errors = query_threads(current_workspace)
    if not sessions and current_workspace:
        all_sessions, all_errors = query_threads(None)
        sessions = all_sessions
        errors.extend(all_errors)
    current = sessions[0] if sessions else None
    parsed = parse_rollout(current.get("rollout_path") if current else None)
    if parsed.get("error"):
        errors.append(f"activity: {parsed['error']}")
    quota = normalize_quota(parsed.get("rate_limits"))
    if quota:
        quota["observed_at"] = iso_time(now)
    last_event = parsed.get("last_event") or 0
    if parsed.get("last_user") and parsed.get("last_user") > (parsed.get("last_final") or 0):
        state = "running"
    elif parsed.get("last_final"):
        state = "ready"
    elif last_event and now - last_event < 45:
        state = "live"
    else:
        state = "unknown"
    return {
        "generated_at": iso_time(now),
        "source": {"state": "ok" if current or quota or parsed.get("activity") else "partial", "codex_home": str(codex_home())},
        "workspace": str(current_workspace) if current_workspace else None,
        "task": {
            "state": state,
            "thread_id": current.get("id") if current else None,
            "title": current.get("title") if current else None,
            "preview": current.get("preview") if current else None,
            "updated_at": current.get("updated_at") if current else None,
            "goal": query_goal(current.get("id") if current else None),
        },
        "quota": {"state": "ok", **quota} if quota else {"state": "unavailable"},
        "activity": {"state": "ok" if parsed.get("activity") else "unavailable", "items": parsed.get("activity", [])},
        "sessions": {"state": "ok" if sessions else "unavailable", "count": len(sessions), "items": sessions[:MAX_SESSIONS]},
        "errors": errors,
    }


INDEX_HTML = """<!doctype html>
<meta charset=utf-8><meta name=viewport content='width=device-width,initial-scale=1'>
<title>Codex 状态</title>
<style>
body{font:16px system-ui,sans-serif;max-width:760px;margin:0 auto;padding:24px;background:#eee;color:#222}
h1{margin:0 0 4px}small{color:#666}.grid{display:grid;gap:12px;margin-top:20px}
section{background:#fff;border:1px solid #999;border-radius:12px;padding:16px}
.value{font-size:28px;font-weight:700;margin:8px 0}.warn{color:#9a4d00}.muted{color:#666}
li{margin:8px 0}.bar{height:12px;border:1px solid #777;border-radius:8px;overflow:hidden}.fill{height:100%;background:#b66b2c}
</style>
<h1>Codex 状态</h1><small id=meta>读取中…</small>
<div class=grid><section><h2>当前任务</h2><div id=task class=value>读取中…</div><div id=task-detail class=muted></div></section>
<section><h2>额度</h2><div id=quota>读取中…</div></section>
<section><h2>最近活动</h2><ul id=activity><li>读取中…</li></ul></section>
<section><h2>会话</h2><div id=sessions>读取中…</div></section></div>
<script>
const esc=s=>String(s??'').replace(/[&<>"']/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
async function refresh(){try{const s=await fetch('/api/status',{cache:'no-store'}).then(r=>r.json());
document.querySelector('#meta').textContent=`${esc(s.source.codex_home)} · ${esc(s.generated_at)}`;
document.querySelector('#task').textContent=esc(s.task.title||'暂无任务')+' · '+esc(s.task.state);
document.querySelector('#task-detail').textContent=esc(s.task.preview||s.workspace||'');
document.querySelector('#quota').innerHTML=s.quota.windows?.length?s.quota.windows.map(w=>`<div><b>${esc(w.name)}</b> · 剩余 ${esc(w.remaining_percent)}%</div><div class=bar><div class=fill style="width:${Math.max(0,Math.min(100,w.remaining_percent||0))}%"></div></div>`).join(''):'<span class=warn>暂无额度快照</span>';
document.querySelector('#activity').innerHTML=s.activity.items.length?s.activity.items.slice().reverse().slice(0,20).map(a=>`<li>${esc(a.message||a.kind)} <small>${esc(a.occurred_at)}</small></li>`).join(''):'<li class=muted>暂无活动</li>';
document.querySelector('#sessions').textContent=`${esc(s.sessions.count)} 个会话`;
}catch(e){document.querySelector('#meta').textContent='采集器不可用';}}
refresh();setInterval(refresh,3000);
</script>"""


class Handler(BaseHTTPRequestHandler):
    def log_message(self, fmt: str, *args: Any) -> None:
        return

    def send_bytes(self, payload: bytes, content_type: str, status: int = 200) -> None:
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Cache-Control", "no-store")
        self.send_header("Access-Control-Allow-Origin", "*")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        try:
            self.wfile.write(payload)
        except (BrokenPipeError, ConnectionResetError):
            pass

    def do_GET(self) -> None:  # noqa: N802
        path = urlparse(self.path).path
        if path == "/":
            self.send_bytes(INDEX_HTML.encode(), "text/html; charset=utf-8")
            return
        if path == "/healthz":
            self.send_bytes(json.dumps({"ok": True, "generated_at": iso_time(time.time())}).encode(), "application/json")
            return
        if path == "/api/status":
            self.send_bytes(json.dumps(build_status(), ensure_ascii=False).encode(), "application/json; charset=utf-8")
            return
        if path == "/api/sessions":
            query = parse_qs(urlparse(self.path).query)
            selected = Path(query["workspace"][0]).expanduser().resolve() if query.get("workspace") else workspace()
            sessions, errors = query_threads(selected)
            self.send_bytes(json.dumps({"items": sessions, "errors": errors}, ensure_ascii=False).encode(), "application/json; charset=utf-8")
            return
        self.send_bytes(b"Not found", "text/plain; charset=utf-8", 404)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--host", default=os.environ.get("CODEX_STATUS_HOST", "127.0.0.1"))
    parser.add_argument("--port", type=int, default=int(os.environ.get("CODEX_STATUS_PORT", "8765")))
    args = parser.parse_args(argv)
    server = ThreadingHTTPServer((args.host, args.port), Handler)
    print(f"Codex status: http://{args.host}:{args.port}/", flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        return 0
    finally:
        server.server_close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
