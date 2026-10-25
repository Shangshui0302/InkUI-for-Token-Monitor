import contextlib
import importlib.util
import json
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
import threading
import tempfile
import sqlite3
import unittest
from urllib.error import HTTPError
from urllib.request import Request, urlopen


spec = importlib.util.spec_from_file_location("eink", Path(__file__).with_name("token-monitor-eink.py"))
eink = importlib.util.module_from_spec(spec)
spec.loader.exec_module(eink)


@contextlib.contextmanager
def running(server):
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield "http://127.0.0.1:" + str(server.server_port)
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=2)


class ViewerTests(unittest.TestCase):
    def setUp(self):
        self.requests = []
        self.upstream_status = 200
        self.upstream_body = json.dumps({"devices": [], "limits": {"providers": []}}).encode()
        test = self

        class Upstream(BaseHTTPRequestHandler):
            def log_message(self, *_args):
                pass

            def do_GET(self):
                test.requests.append((self.path, self.headers.get("Authorization")))
                self.send_response(test.upstream_status)
                if test.upstream_status == 302:
                    self.send_header("Location", "/credential-destination")
                self.end_headers()
                self.wfile.write(test.upstream_body)

        self.stack = contextlib.ExitStack()
        self.addCleanup(self.stack.close)
        self.upstream_url = self.stack.enter_context(running(ThreadingHTTPServer(("127.0.0.1", 0), Upstream)))
        self.url = self.stack.enter_context(running(eink.make_server("127.0.0.1", 0, self.upstream_url, "hub-test-secret", "viewer-test-token")))

    def request(self, path="/api/stats", method="GET", token="viewer-test-token"):
        request = Request(self.url + path, method=method, headers={"Authorization": "Bearer " + token})
        try:
            response = urlopen(request, timeout=3)
        except HTTPError as error:
            response = error
        with response:
            return response.status, response.read(), response.headers

    def test_viewer_auth_is_separate_from_hub_auth(self):
        for token in ("", "wrong", "hub-test-secret"):
            self.assertEqual(self.request(token=token)[0], 401)
        self.assertEqual(self.requests, [])
        status, body, headers = self.request()
        self.assertEqual(status, 200)
        self.assertEqual(json.loads(body)["limits"]["providers"], [])
        self.assertEqual(self.requests, [("/api/stats", "Bearer hub-test-secret")])
        self.assertNotIn(b"hub-test-secret", body)
        self.assertEqual(headers["Cache-Control"], "no-store")

    def test_only_fixed_read_route_is_forwarded(self):
        for path in ("/api/ingest", "/api/sync/settings/modelAliases", "/../README.md", "/%2e%2e/README.md", "/api/stats?secret=anything"):
            self.assertEqual(self.request(path)[0], 404)
        for method in ("POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"):
            self.assertEqual(self.request(method=method)[0], 405)
        self.assertEqual(self.requests, [])

    def test_assets_and_health_do_not_contact_hub(self):
        for path in eink.ASSETS:
            status, body, headers = self.request(path, token="")
            self.assertEqual(status, 200, path)
            self.assertTrue(body)
            self.assertIn("default-src 'self'", headers["Content-Security-Policy"])
        status, body, _ = self.request("/healthz", token="")
        self.assertEqual(status, 200)
        self.assertEqual(json.loads(body)["role"], "eink-viewer")
        self.assertEqual(self.requests, [])

    def test_upstream_auth_error_does_not_expose_body(self):
        self.upstream_status = 401
        self.upstream_body = b"private upstream diagnostic"
        status, body, _ = self.request()
        self.assertEqual(status, 502)
        self.assertEqual(json.loads(body)["error"], "hub_unauthorized")
        self.assertNotIn(b"private", body)

    def test_history_is_authenticated_and_read_only(self):
        self.upstream_body = json.dumps({"daily": [{"date": "2026-01-01", "tokens": 8}],
                                         "monthly": [], "summary": {"activeDays": 1}}).encode()
        for token in ("", "wrong", "hub-test-secret"):
            self.assertEqual(self.request("/api/history", token=token)[0], 401)
        self.assertEqual(self.requests, [])
        status, body, _ = self.request("/api/history")
        self.assertEqual(status, 200)
        self.assertEqual(json.loads(body)["daily"][0]["tokens"], 8)
        self.assertEqual(self.requests, [("/api/history", "Bearer hub-test-secret")])
        for method in ("POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"):
            self.assertEqual(self.request("/api/history", method=method)[0], 405)
        self.assertEqual(self.request("/api/history?url=http://example.com")[0], 404)
        self.assertEqual(self.request("/api/devices")[0], 404)
        self.assertEqual(len(self.requests), 1)

    def test_invalid_history_is_not_a_successful_snapshot(self):
        for value in ({}, {"daily": {}, "monthly": [], "summary": {}},
                      {"daily": [], "monthly": [], "summary": []}):
            self.upstream_body = json.dumps(value).encode()
            status, body, _ = self.request("/api/history")
            self.assertEqual(status, 502)
            self.assertEqual(json.loads(body)["error"], "hub_invalid_response")

    def test_archive_reads_committed_wal_without_exposing_raw_paths(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "archive.sqlite"
            with contextlib.closing(sqlite3.connect(path)) as writer:
                writer.execute("PRAGMA journal_mode=WAL")
                writer.execute("CREATE TABLE metadata(key TEXT PRIMARY KEY, value TEXT)")
                writer.execute("CREATE TABLE sessions(session_key TEXT PRIMARY KEY, entry_json TEXT)")
                writer.executemany("INSERT INTO metadata VALUES (?,?)", [('schema-version', '1'), ('legacy-migrated', '1')])
                entry = {"capturedAt": "2026-10-23T08:00:00Z", "periods": {"allTime": {
                    "client": "codex", "totalTokens": 90, "costUsd": 1, "title": "Example",
                    "path": "/private/source", "turnEnded": False}}}
                writer.execute("INSERT INTO sessions VALUES (?,?)", ('codex:example', json.dumps(entry)))
                writer.commit()
                self.assertTrue(Path(str(path) + '-wal').exists())
                with running(eink.make_server("127.0.0.1", 0, self.upstream_url, "hub-test-secret", "viewer-test-token", session_archive=path)) as url:
                    for token in ('', 'hub-test-secret'):
                        with self.assertRaises(HTTPError) as error:
                            urlopen(Request(url + '/api/session-archive', headers={'Authorization': 'Bearer ' + token}))
                        error.exception.close()
                        self.assertEqual(error.exception.code, 401)
                    with urlopen(Request(url + '/api/session-archive', headers={'Authorization': 'Bearer viewer-test-token'})) as response:
                        data = json.load(response)
                    self.assertEqual(data['sessions']['codex:example']['totalTokens'], 90)
                    self.assertTrue(data['sessions']['codex:example']['archived'])
                    self.assertEqual(data['scope'], 'local-device')
                    self.assertNotIn('path', data['sessions']['codex:example'])
                self.assertEqual(self.requests, [])
                self.assertEqual(json.loads(writer.execute('SELECT entry_json FROM sessions').fetchone()[0]), entry)

    def test_missing_archive_does_not_create_a_database(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'missing.sqlite'
            with self.assertRaises(sqlite3.Error):
                eink.read_session_archive(path)
            self.assertFalse(path.exists())
        self.assertEqual(self.request('/api/session-archive')[0], 503)
        self.assertEqual(self.request('/api/session-archive', method='POST')[0], 405)
        self.assertEqual(self.requests, [])

    def test_redirect_never_receives_hub_secret(self):
        self.upstream_status = 302
        self.assertEqual(self.request()[0], 502)
        self.assertEqual(len(self.requests), 1)

    def test_malformed_and_unexpected_payloads_are_not_success(self):
        for body in (b"not json", b"[]", b'{}', b'{"devices":[],"limits":{"providers":[]},"bad":NaN}'):
            self.upstream_body = body
            status, data, _ = self.request()
            self.assertEqual(status, 502)
            self.assertEqual(json.loads(data)["error"], "hub_invalid_response")

    def test_missing_hub_returns_unavailable(self):
        closed = ThreadingHTTPServer(("127.0.0.1", 0), BaseHTTPRequestHandler)
        port = closed.server_port
        closed.server_close()
        with running(eink.make_server("127.0.0.1", 0, f"http://127.0.0.1:{port}")) as url:
            with self.assertRaises(HTTPError) as result:
                urlopen(url + "/api/stats", timeout=3)
            with result.exception as response:
                self.assertEqual(response.code, 503)
                self.assertEqual(json.load(response)["error"], "hub_unavailable")

    def test_lan_binding_and_configuration_boundaries(self):
        with self.assertRaisesRegex(ValueError, "Non-loopback"):
            eink.make_server("0.0.0.0", 0, self.upstream_url)
        with self.assertRaisesRegex(ValueError, "separate view token"):
            eink.make_server("127.0.0.1", 0, self.upstream_url, "same", "same")
        for url in ("file:///tmp/data", "http://user:secret@localhost", "http://localhost/api/stats", "http://localhost?key=secret", "http://localhost:bad"):
            with self.assertRaises(ValueError):
                eink.validate_hub_url(url)

    def test_existing_credentials_file_reads_only_host_secret(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "credentials.json"
            path.write_text(json.dumps({"credentials": {"hub": {"hostSecret": "hub-only"},
                                                       "providers": {"key": "not-for-viewer"}}}))
            self.assertEqual(eink.read_hub_credentials(path), "hub-only")
            for value in ("invalid private content", "{}", '{"credentials":{"hub":{"hostSecret":4}}}'):
                path.write_text(value)
                with self.assertRaisesRegex(ValueError, "no valid Hub host secret") as error:
                    eink.read_hub_credentials(path)
                self.assertNotIn("private content", str(error.exception))


if __name__ == "__main__":
    unittest.main()
