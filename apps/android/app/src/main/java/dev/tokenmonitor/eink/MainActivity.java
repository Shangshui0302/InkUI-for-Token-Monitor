package dev.tokenmonitor.eink;

import android.app.Activity;
import android.content.res.Configuration;
import android.graphics.Bitmap;
import android.os.Bundle;
import android.view.View;
import android.webkit.CookieManager;
import android.webkit.JavascriptInterface;
import android.webkit.WebResourceRequest;
import android.webkit.WebResourceResponse;
import android.webkit.WebSettings;
import android.webkit.WebView;
import android.webkit.WebViewClient;
import android.widget.FrameLayout;
import org.json.JSONObject;
import java.io.ByteArrayInputStream;
import java.io.InputStream;
import java.net.HttpURLConnection;
import java.net.Proxy;
import java.net.URL;
import java.nio.charset.StandardCharsets;
import java.util.HashMap;
import java.util.Map;
import java.util.Set;
import java.util.concurrent.ConcurrentHashMap;

/** The WebView can read packaged UI and two native GET routes only. */
public final class MainActivity extends Activity {
    private WebView webView;
    private ConnectionStore connections;
    private DisplayRefreshController displayRefresh;
    private FullscreenController fullscreen;
    private DeviceStatusController deviceStatus;
    private volatile boolean foreground;
    private ConnectionTestController connectionTests;
    private long connectionTestGeneration;
    private final Set<HttpURLConnection> activeRequests = ConcurrentHashMap.newKeySet();

    @Override public void onCreate(Bundle state) {
        super.onCreate(state);
        connections = new ConnectionStore(this);
        connectionTests = new ConnectionTestController((body, source) -> {
            validateResponse(body, "/api/stats");
        }, MainActivity::classifyConnectionFailure);
        webView = new WebView(this);
        displayRefresh = new DisplayRefreshController(webView);
        WebView.setWebContentsDebuggingEnabled(BuildConfig.DEBUG);
        WebSettings settings = webView.getSettings();
        settings.setJavaScriptEnabled(true);
        settings.setDomStorageEnabled(true);
        settings.setAllowFileAccess(false);
        settings.setAllowContentAccess(false);
        settings.setAllowFileAccessFromFileURLs(false);
        settings.setAllowUniversalAccessFromFileURLs(false);
        settings.setMixedContentMode(WebSettings.MIXED_CONTENT_NEVER_ALLOW);
        settings.setJavaScriptCanOpenWindowsAutomatically(false);
        settings.setSupportMultipleWindows(false);
        settings.setMediaPlaybackRequiresUserGesture(true);
        settings.setGeolocationEnabled(false);
        settings.setSaveFormData(false);
        settings.setCacheMode(WebSettings.LOAD_NO_CACHE);
        // Native fixed GETs also have no process-level cookie jar.
        java.net.CookieHandler.setDefault(null);
        CookieManager.getInstance().setAcceptCookie(false);
        CookieManager.getInstance().setAcceptThirdPartyCookies(webView, false);
        webView.setVerticalScrollBarEnabled(false);
        webView.setHorizontalScrollBarEnabled(false);
        webView.setOverScrollMode(View.OVER_SCROLL_NEVER);
        webView.addJavascriptInterface(new ConnectionBridge(), "TokenMonitorAndroid");
        webView.setWebViewClient(new LocalClient());
        FrameLayout viewport = new FrameLayout(this);
        viewport.addView(webView, new FrameLayout.LayoutParams(
                FrameLayout.LayoutParams.MATCH_PARENT, FrameLayout.LayoutParams.MATCH_PARENT));
        setContentView(viewport);
        fullscreen = new FullscreenController(this, viewport, enabled -> deviceStatus.setFullscreen(enabled));
        deviceStatus = new DeviceStatusController(this, fullscreen.isEnabled(), this::dispatchDeviceStatus);
        if (android.os.Build.VERSION.SDK_INT >= 33) {
            getOnBackInvokedDispatcher().registerOnBackInvokedCallback(
                    android.window.OnBackInvokedDispatcher.PRIORITY_DEFAULT, this::handleBack);
        }
        webView.loadUrl(NativePolicy.LOCAL_ORIGIN + "/index.html");
    }

    private final class ConnectionBridge {
        @JavascriptInterface public String getConnection() {
            try { return connections.snapshot().publicJson().toString(); }
            catch (Exception ignored) { return "{}"; }
        }
        @JavascriptInterface public String getAppInfo() {
            try {
                JSONObject result = new JSONObject();
                result.put("versionName", BuildConfig.VERSION_NAME);
                result.put("versionCode", BuildConfig.VERSION_CODE);
                return result.toString();
            } catch (Exception ignored) { return "{}"; }
        }
        @JavascriptInterface public String saveConnection(String json) { return connections.save(json); }
        @JavascriptInterface public void testConnection(String payloadJson, String requestId) {
            if (requestId == null || requestId.length() > 64 || !requestId.matches("[A-Za-z0-9_-]{1,64}")) return;
            runOnUiThread(() -> startConnectionTest(payloadJson, requestId));
        }
        @JavascriptInterface public void cancelConnectionTest() {
            runOnUiThread(() -> connectionTests.cancel());
        }
        @JavascriptInterface public String getDisplayCapabilities() { return displayRefresh.capabilities(); }
        @JavascriptInterface public String getDeviceStatus() { return deviceStatus.snapshotJson(); }
        @JavascriptInterface public void setFullscreen(boolean enabled) {
            runOnUiThread(() -> { if (!isDestroyed()) fullscreen.setEnabled(enabled); });
        }
        @JavascriptInterface public void requestDisplayRefresh(String reason) {
            runOnUiThread(() -> displayRefresh.request(reason));
        }
    }

    private final class LocalClient extends WebViewClient {
        @Override public void onPageStarted(WebView view, String url, Bitmap favicon) { deviceStatus.pageLoading(); }
        @Override public boolean shouldOverrideUrlLoading(WebView view, WebResourceRequest request) { return true; }
        @Override public boolean shouldOverrideUrlLoading(WebView view, String url) { return true; }
        @Override public WebResourceResponse shouldInterceptRequest(WebView view, WebResourceRequest request) {
            if (!NativePolicy.isLocalRequest(request.getUrl().toString())) return error(403, "request_blocked");
            if (!"GET".equals(request.getMethod())) return error(405, "method_not_allowed");
            String path = request.getUrl().getEncodedPath();
            if (NativePolicy.ROUTES.contains(path)) return proxy(path);
            if (!NativePolicy.ASSETS.contains(path)) return error(404, "not_found");
            try {
                String mime = path.endsWith(".css") ? "text/css" : path.endsWith(".html") ? "text/html" : "application/javascript";
                return response(200, mime, getAssets().open(path.substring(1)));
            } catch (Exception ignored) { return error(503, "ui_unavailable"); }
        }
        @Override public void onPageFinished(WebView view, String url) {
            dispatchVisibility();
            deviceStatus.pageReady();
            displayRefresh.request("content");
        }
    }

    private WebResourceResponse proxy(String path) {
        ConnectionStore.Config config = connections.snapshot();
        if (!foreground) return error(503, "app_background");
        if (!NativePolicy.ROUTES.contains(path)) return error(404, "not_found");
        String origin = config.hubUrl;
        String secret = config.hubSecret;
        if (origin.isEmpty()) return error(503, "hub_unconfigured");
        HttpURLConnection upstream = null;
        WebResourceResponse result;
        try {
            upstream = (HttpURLConnection) new URL(origin + path).openConnection(Proxy.NO_PROXY);
            upstream.setInstanceFollowRedirects(false);
            upstream.setConnectTimeout(8000);
            upstream.setReadTimeout(8000);
            upstream.setUseCaches(false);
            upstream.setRequestMethod("GET");
            upstream.setRequestProperty("Accept", "application/json");
            upstream.setRequestProperty("Accept-Encoding", "identity");
            if (!secret.isEmpty()) upstream.setRequestProperty("Authorization", "Bearer " + secret);
            activeRequests.add(upstream);
            if (!foreground || !connections.isCurrent(config)) return error(409, "connection_changed");
            int status = upstream.getResponseCode();
            // WebResourceResponse cannot represent redirects; never expose Location or a remote body.
            if (status >= 300 && status <= 399) result = error(502, "upstream_redirect_blocked");
            else if (status == 401 || status == 403) result = error(502, "hub_unauthorized");
            else if (status != 200) result = error(502, "hub_http_error");
            else if (upstream.getContentLengthLong() > NativePolicy.MAX_RESPONSE_BYTES) result = error(502, "response_too_large");
            else {
                byte[] body;
                try (InputStream in = upstream.getInputStream()) { body = NativePolicy.readLimited(in); }
                JSONObject data = validateResponse(body, path);
                result = json(200, data.toString());
            }
        } catch (org.json.JSONException | IllegalArgumentException ignored) { result = error(502, "invalid_response"); }
        catch (Exception failure) { result = error(503, "response_limit".equals(failure.getMessage())
                ? "response_too_large" : "hub_" + classifyConnectionFailure(failure)); }
        finally {
            if (upstream != null) { activeRequests.remove(upstream); upstream.disconnect(); }
        }
        if (!connections.isCurrent(config)) return error(409, "connection_changed");
        if (!foreground) return error(503, "app_background");
        return result;
    }

    private static JSONObject validateResponse(byte[] body, String path) throws org.json.JSONException {
        JSONObject data = new JSONObject(new String(body, StandardCharsets.UTF_8));
        if (path.equals("/api/stats") && (data.optJSONArray("devices") == null
                || data.optJSONObject("limits") == null || data.getJSONObject("limits").optJSONArray("providers") == null)) throw new IllegalArgumentException();
        if (path.equals("/api/history") && (data.optJSONArray("daily") == null
                || data.optJSONArray("monthly") == null || data.optJSONObject("summary") == null)) throw new IllegalArgumentException();
        return data;
    }

    private static String classifyConnectionFailure(Exception failure) {
        for (Throwable cause = failure; cause != null; cause = cause.getCause()) {
            if (cause instanceof android.system.ErrnoException
                    && ((android.system.ErrnoException) cause).errno == android.system.OsConstants.ECONNREFUSED) return "connection_refused";
        }
        return ConnectionTestController.classifyFailure(failure);
    }

    private void startConnectionTest(String input, String requestId) {
        if (isDestroyed()) return;
        long generation = ++connectionTestGeneration;
        connectionTests.cancel();
        String source = "hub";
        String origin = "";
        try {
            if (input == null || input.length() > 24576) throw new IllegalArgumentException();
            JSONObject data = new JSONObject(input);
            Object selected = data.opt("source");
            if (!"hub".equals(selected)) throw new IllegalArgumentException();
            Object selectedUrl = data.opt("hubUrl");
            if (!(selectedUrl instanceof String)) throw new IllegalArgumentException();
            origin = NativePolicy.normalizeOrigin((String) selectedUrl);
            // Parse through the same origin/credential policy as saving, without changing the store.
            ConnectionStore.Config candidate = connections.candidate(input);
            String secret = candidate.hubSecret;
            if (!foreground) return;
            connectionTests.start(requestId, source, origin, secret,
                    () -> foreground && connections.isCurrent(candidate),
                    result -> runOnUiThread(() -> {
                        if (generation != connectionTestGeneration || !foreground || isDestroyed()) return;
                        // Saving can race with a worker finishing or its queued UI callback.
                        String error = connections.isCurrent(candidate) ? result.error : "cancelled";
                        dispatchConnectionTest(new ConnectionTestController.Result(result.id, result.source,
                                result.origin, error, result.latencyMs));
                    }));
        } catch (Exception invalid) {
            if (foreground) dispatchConnectionTest(new ConnectionTestController.Result(
                    requestId, source, origin, "invalid_connection", 0));
        }
    }

    private void dispatchConnectionTest(ConnectionTestController.Result result) {
        if (!foreground || webView == null) return;
        try {
            JSONObject detail = new JSONObject().put("id", result.id).put("ok", result.ok)
                    .put("source", result.source).put("origin", result.origin).put("latencyMs", result.latencyMs);
            if (result.error != null) detail.put("error", result.error);
            webView.evaluateJavascript("window.dispatchEvent(new CustomEvent('tokenmonitor:connection-test',{detail:"
                    + detail + "}))", null);
        } catch (org.json.JSONException ignored) { }
    }

    private static WebResourceResponse error(int status, String code) {
        return json(status, "{\"error\":\"" + code + "\"}");
    }
    private static WebResourceResponse json(int status, String body) {
        return response(status, "application/json", new ByteArrayInputStream(body.getBytes(StandardCharsets.UTF_8)));
    }
    private static WebResourceResponse response(int status, String mime, InputStream data) {
        Map<String, String> headers = new HashMap<>();
        headers.put("Cache-Control", "no-store"); headers.put("X-Content-Type-Options", "nosniff");
        headers.put("Referrer-Policy", "no-referrer");
        headers.put("Content-Security-Policy", "default-src 'self'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self'; object-src 'none'; frame-src 'none'; worker-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'");
        return new WebResourceResponse(mime, "UTF-8", status, status == 200 ? "OK" : "Request failed", headers, data);
    }

    private void dispatchVisibility() {
        if (webView != null) webView.evaluateJavascript("window.dispatchEvent(new CustomEvent('tokenmonitor:visibility',{detail:{hidden:"
                + (!foreground) + "}}))", null);
    }
    private void dispatchDeviceStatus(String json) {
        if (!foreground || webView == null) return;
        webView.evaluateJavascript("window.dispatchEvent(new CustomEvent('tokenmonitor:device-status',{detail:"
                + json + "}))", null);
    }
    @Override protected void onResume() {
        super.onResume(); foreground = true;
        if (webView != null) {
            webView.onResume();
            displayRefresh.resume();
            fullscreen.resume();
            deviceStatus.start();
            dispatchVisibility();
        }
    }
    @Override protected void onPause() {
        foreground = false; ++connectionTestGeneration; dispatchVisibility();
        if (connectionTests != null) connectionTests.cancel();
        if (deviceStatus != null) deviceStatus.stop();
        if (fullscreen != null) fullscreen.pause();
        if (displayRefresh != null) displayRefresh.pause();
        for (HttpURLConnection request : activeRequests) request.disconnect();
        if (webView != null) webView.onPause();
        super.onPause();
    }
    @Override public void onWindowFocusChanged(boolean hasFocus) {
        super.onWindowFocusChanged(hasFocus);
        if (fullscreen != null) fullscreen.focusChanged(hasFocus);
    }
    @Override public void onConfigurationChanged(Configuration configuration) {
        super.onConfigurationChanged(configuration);
        if (fullscreen != null) fullscreen.configurationChanged();
    }
    @Override public void onBackPressed() {
        handleBack();
    }
    private void handleBack() {
        webView.evaluateJavascript("Boolean(window.tokenMonitorBack?.())", handled -> {
            if (!"true".equals(handled)) moveTaskToBack(true);
        });
    }
    @Override protected void onDestroy() {
        foreground = false; ++connectionTestGeneration;
        if (connectionTests != null) connectionTests.destroy();
        if (deviceStatus != null) deviceStatus.stop();
        if (fullscreen != null) fullscreen.destroy();
        if (displayRefresh != null) displayRefresh.destroy();
        if (webView != null) {
            webView.removeJavascriptInterface("TokenMonitorAndroid"); webView.stopLoading(); webView.destroy(); webView = null;
        }
        super.onDestroy();
    }
}
