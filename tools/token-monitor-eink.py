#!/usr/bin/env python3
"""Serve the e-ink dashboard and fixed read-only Token Monitor Hub endpoints."""

import argparse
import hmac
import ipaddress
import json
import os
import sqlite3
from contextlib import closing
from pathlib import Path
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import sys
from urllib.error import HTTPError, URLError
from urllib.parse import urlsplit
from urllib.request import HTTPRedirectHandler, ProxyHandler, Request, build_opener


UI_ROOT = Path(__file__).resolve().parents[1] / "apps" / "eink"
MAX_RESPONSE_BYTES = 8 * 1024 * 1024
ASSETS = {
    "/": ("index.html", "text/html; charset=utf-8"),
    "/index.html": ("index.html", "text/html; charset=utf-8"),
    "/themes.css": ("themes.css", "text/css; charset=utf-8"),
    "/theme.mjs": ("theme.mjs", "text/javascript; charset=utf-8"),
    "/styles.css": ("styles.css", "text/css; charset=utf-8"),
    "/app.js": ("app.js", "text/javascript; charset=utf-8"),
    "/model.mjs": ("model.mjs", "text/javascript; charset=utf-8"),
    "/dashboard-model.mjs": ("dashboard-model.mjs", "text/javascript; charset=utf-8"),
    "/dashboard.css": ("dashboard.css", "text/css; charset=utf-8"),
    "/dashboard.js": ("dashboard.js", "text/javascript; charset=utf-8"),
}
READ_ROUTES = frozenset(("/api/stats", "/api/history", "/api/session-archive"))
ARCHIVE_FIELDS = frozenset((
    "client", "title", "projectLabel", "sessionKind", "totalTokens", "costUsd",
    "inputTokens", "outputTokens", "cacheReadTokens", "cacheWriteTokens", "reasoningTokens",
    "messageCount", "unpricedTokens", "startedAt", "lastUsedAt", "models", "modelCosts",
    "contextTokens", "contextWindow", "turnEnded", "archived", "deleted", "sourceDeleted",
))


class NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        # Never forward the Hub's credential to a redirect destination.
        return None


def validate_hub_url(value):
    parsed = urlsplit(value)
    if (parsed.scheme not in ("http", "https") or not parsed.hostname
            or parsed.username is not None or parsed.password is not None
            or parsed.path not in ("", "/") or parsed.query or parsed.fragment):
        raise ValueError("Hub URL must be an http(s) origin without credentials or a path")
    _ = parsed.port  # Reject malformed ports before starting the server.
    return value.rstrip("/") + "/api/stats"


def is_loopback(host):
    if host == "localhost":
        return True
    try:
        return ipaddress.ip_address(host).is_loopback
    except ValueError:
        return False


def read_secret(path, env_name):
    value = Path(path).read_text().strip() if path else os.environ.get(env_name, "").strip()
    if "\r" in value or "\n" in value:
        raise ValueError("Credentials must be a single line")
    return value


def read_hub_credentials(path):
    """Read only the existing Hub host secret; never copy the provider store."""
    try:
        data = json.loads(Path(path).read_text())
        value = data["credentials"]["hub"]["hostSecret"]
        if not isinstance(value, str) or not value.strip() or "\r" in value or "\n" in value:
            raise ValueError()
        return value.strip()
    except (KeyError, TypeError, ValueError):
        raise ValueError("Token Monitor credentials file has no valid Hub host secret") from None


def read_session_archive(path):
    # mode=ro observes committed WAL changes without running migrations or a checkpoint.
    uri = Path(path).resolve().as_uri() + "?mode=ro"
    with closing(sqlite3.connect(uri, uri=True, timeout=2)) as connection:
        connection.execute("PRAGMA query_only = ON")
        connection.execute("BEGIN")
        metadata = dict(connection.execute("SELECT key,value FROM metadata WHERE key IN ('schema-version','legacy-migrated')"))
        if metadata.get("schema-version") != "1" or metadata.get("legacy-migrated") != "1":
            raise ValueError("Unsupported session archive format")
        sessions = {}
        captured_at = None
        for key, encoded in connection.execute("SELECT session_key,entry_json FROM sessions"):
            entry = json.loads(encoded)
            session = entry.get("periods", {}).get("allTime")
            if not isinstance(session, dict):
                continue
            sessions[key] = {name: value for name, value in session.items() if name in ARCHIVE_FIELDS}
            # An archived observation is not evidence of a currently running task.
            sessions[key]["archived"] = True
            captured = entry.get("capturedAt")
            if isinstance(captured, str) and (captured_at is None or captured > captured_at):
                captured_at = captured
        return {"available": True, "scope": "local-device", "source": "token-monitor-archive",
                "capturedAt": captured_at, "sessions": sessions}


def make_server(host, port, hub_url, hub_secret="", view_token="", ui_root=UI_ROOT, session_archive=None):
    if not is_loopback(host) and not view_token:
        raise ValueError("Non-loopback binding requires EINK_VIEW_TOKEN or --view-token-file")
    if hub_secret and view_token and hmac.compare_digest(hub_secret.encode(), view_token.encode()):
        raise ValueError("Use a separate view token, different from the Hub secret")
    upstream_origin = validate_hub_url(hub_url).removesuffix("/api/stats")
    # Local data must not accidentally transit an environment-configured proxy.
    opener = build_opener(ProxyHandler({}), NoRedirect())

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_args):
            pass  # Do not log URLs or credentials supplied by clients.

        def send_body(self, status, body, content_type="application/json; charset=utf-8", head=False):
            self.send_response(status)
            self.send_header("Content-Type", content_type)
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Cache-Control", "no-store")
            self.send_header("X-Content-Type-Options", "nosniff")
            self.send_header("Referrer-Policy", "no-referrer")
            self.send_header("Content-Security-Policy", "default-src 'self'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'")
            self.end_headers()
            if not head:
                try:
                    self.wfile.write(body)
                except (BrokenPipeError, ConnectionResetError):
                    pass

        def send_error_json(self, status, code, message):
            body = json.dumps({"error": code, "message": message}, ensure_ascii=False).encode()
            self.send_body(status, body)

        def do_GET(self):
            self.serve()

        def do_HEAD(self):
            self.serve(head=True)

        def serve(self, head=False):
            if self.path in ASSETS:
                filename, content_type = ASSETS[self.path]
                try:
                    content = (ui_root / filename).read_bytes()
                except OSError:
                    return self.send_error_json(503, "ui_missing", "界面文件缺失，请检查启动目录。")
                return self.send_body(200, content, content_type, head)
            if self.path == "/healthz":
                return self.send_body(200, b'{"ok":true,"role":"eink-viewer"}', head=head)
            if self.path not in READ_ROUTES:
                return self.send_error_json(404, "not_found", "没有这个页面或接口。")
            if head:
                return self.send_error_json(405, "method_not_allowed", "统计接口只支持读取。")
            supplied = self.headers.get("Authorization", "")
            expected = "Bearer " + view_token
            if view_token and not hmac.compare_digest(supplied.encode(), expected.encode()):
                return self.send_error_json(401, "unauthorized", "请填写正确的查看令牌。")
            if self.path == "/api/session-archive":
                if not session_archive:
                    return self.send_error_json(503, "archive_unconfigured", "本机累计会话归档尚未配置。")
                try:
                    data = read_session_archive(session_archive)
                    content = json.dumps(data, ensure_ascii=False, allow_nan=False).encode()
                    if len(content) > MAX_RESPONSE_BYTES:
                        raise ValueError("Archive too large")
                    return self.send_body(200, content)
                except (sqlite3.Error, OSError, ValueError, TypeError, AttributeError):
                    return self.send_error_json(503, "archive_unavailable", "本机累计会话归档暂时不可用。")
            headers = {"Accept": "application/json"}
            if hub_secret:
                headers["Authorization"] = "Bearer " + hub_secret
            request = Request(upstream_origin + self.path, headers=headers, method="GET")
            try:
                with opener.open(request, timeout=8) as response:
                    content = response.read(MAX_RESPONSE_BYTES + 1)
                if len(content) > MAX_RESPONSE_BYTES:
                    return self.send_error_json(502, "hub_response_too_large", "数据过大，暂时无法读取。")
                data = json.loads(content)
                if not isinstance(data, dict):
                    raise ValueError("Unexpected response shape")
                if self.path == "/api/stats":
                    if (not isinstance(data.get("devices"), list)
                            or not isinstance(data.get("limits"), dict)
                            or not isinstance(data["limits"].get("providers"), list)):
                        raise ValueError("Unexpected stats shape")
                elif (not isinstance(data.get("daily"), list)
                      or not isinstance(data.get("monthly"), list)
                      or not isinstance(data.get("summary"), dict)):
                    raise ValueError("Unexpected history shape")
                return self.send_body(200, json.dumps(data, ensure_ascii=False, allow_nan=False).encode())
            except HTTPError as error:
                error.close()
                if error.code in (401, 403):
                    return self.send_error_json(502, "hub_unauthorized", "电脑端无法通过 Token Monitor 认证，请检查 Hub 密钥。")
                return self.send_error_json(502, "hub_http_error", "Token Monitor 暂时无法提供数据。")
            except (URLError, OSError, TimeoutError):
                return self.send_error_json(503, "hub_unavailable", "无法连接 Token Monitor，请确认它已启动。")
            except (ValueError, UnicodeError):
                return self.send_error_json(502, "hub_invalid_response", "Token Monitor 返回了无法识别的数据。")

        def reject_write(self):
            self.close_connection = True
            self.send_error_json(405, "method_not_allowed", "此界面只支持读取。")

        do_POST = do_PUT = do_PATCH = do_DELETE = do_OPTIONS = reject_write

    # localhost is normalized to an IPv4 loopback; explicit IPv6 binds are supported.
    if ":" in host:
        import socket

        class IPv6Server(ThreadingHTTPServer):
            address_family = socket.AF_INET6

        server_type = IPv6Server
    else:
        server_type = ThreadingHTTPServer
    return server_type(("127.0.0.1" if host == "localhost" else host, port), Handler)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=8766)
    parser.add_argument("--hub-url", default=os.environ.get("TOKEN_MONITOR_URL", "http://127.0.0.1:17321"))
    hub_auth = parser.add_mutually_exclusive_group()
    hub_auth.add_argument("--hub-secret-file", help="File containing the Hub secret; otherwise TOKEN_MONITOR_SECRET")
    hub_auth.add_argument("--hub-credentials-file", help="Read the existing Token Monitor credentials.json without copying its secrets")
    parser.add_argument("--view-token-file", help="File containing a separate viewer token; otherwise EINK_VIEW_TOKEN")
    parser.add_argument("--session-archive", help="Read an existing Token Monitor session-usage-archive.sqlite in read-only mode")
    args = parser.parse_args()
    try:
        hub_secret = (read_hub_credentials(args.hub_credentials_file) if args.hub_credentials_file
                      else read_secret(args.hub_secret_file, "TOKEN_MONITOR_SECRET"))
        server = make_server(args.host, args.port, args.hub_url,
                             hub_secret,
                             read_secret(args.view_token_file, "EINK_VIEW_TOKEN"),
                             session_archive=args.session_archive)
    except (OSError, ValueError) as error:
        # Input errors are bounded; never include the actual credential content.
        parser.error(str(error))
    address = server.server_address[0]
    display_host = "[" + address + "]" if ":" in address else address
    print(f"E-ink viewer: http://{display_host}:{server.server_address[1]}/", flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()
    return 0


if __name__ == "__main__":
    sys.exit(main())
