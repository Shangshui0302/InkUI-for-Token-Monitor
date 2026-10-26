package dev.tokenmonitor.eink;

import android.app.Activity;
import android.content.res.Configuration;
import android.graphics.Bitmap;
import android.graphics.Color;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.os.SystemClock;
import android.view.Gravity;
import android.view.View;
import android.webkit.CookieManager;
import android.webkit.JavascriptInterface;
import android.webkit.RenderProcessGoneDetail;
import android.webkit.WebResourceError;
import android.webkit.WebResourceRequest;
import android.webkit.WebResourceResponse;
import android.webkit.WebSettings;
import android.webkit.WebView;
import android.webkit.WebViewClient;
import android.widget.Button;
import android.widget.FrameLayout;
import android.widget.LinearLayout;
import android.widget.TextView;
import org.json.JSONObject;
import java.io.ByteArrayInputStream;
import java.io.InputStream;
import java.nio.charset.StandardCharsets;
import java.util.HashMap;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.atomic.AtomicLong;

/** The WebView can read packaged UI and two native GET routes only. */
public final class MainActivity extends Activity {
    private volatile WebView webView;
    private FrameLayout viewport;
    private ConnectionStore connections;
    private DisplayRefreshController displayRefresh;
    private FullscreenController fullscreen;
    private DeviceStatusController deviceStatus;
    private volatile boolean foreground;
    private ConnectionTestController connectionTests;
    private HubHttpClient dataRequests;
    private long connectionTestGeneration;
    private volatile long webGeneration;
    private final AtomicLong connectionEpoch = new AtomicLong();
    private final Handler main = new Handler(Looper.getMainLooper());
    private final RecoveryPermit recoveryPermit = new RecoveryPermit(SystemClock::elapsedRealtime);
    private final WebViewRecoveryPolicy recoveryPolicy = new WebViewRecoveryPolicy(SystemClock::elapsedRealtime);
    // An Abort can arrive before shouldInterceptRequest registers its native operation.
    // Retain a small, expiring tombstone until that operation's total deadline has passed.
    private final ConcurrentHashMap<String, Long> cancelledReads = new ConcurrentHashMap<>();
    private boolean uiReady, recreateOnResume;
    private Runnable startupTimeout;

    @Override public void onCreate(Bundle state) {
        super.onCreate(state);
        connections = new ConnectionStore(this);
        connectionTests = new ConnectionTestController((body, source) -> validateResponse(body, "/api/stats"),
                MainActivity::classifyConnectionFailure);
        dataRequests = new HubHttpClient(MainActivity::validateResponse, MainActivity::classifyConnectionFailure);
        // The native GET transport also has no process-level cookie jar.
        java.net.CookieHandler.setDefault(null);
        viewport = new FrameLayout(this);
        viewport.setBackgroundColor(Color.WHITE);
        setContentView(viewport);
        fullscreen = new FullscreenController(this, viewport,
                enabled -> { if (deviceStatus != null) deviceStatus.setFullscreen(enabled); });
        deviceStatus = new DeviceStatusController(this, fullscreen.isEnabled(), this::dispatchDeviceStatus);
        if (android.os.Build.VERSION.SDK_INT >= 33) {
            getOnBackInvokedDispatcher().registerOnBackInvokedCallback(
                    android.window.OnBackInvokedDispatcher.PRIORITY_DEFAULT, this::handleBack);
        }
        createWebView();
    }

    private boolean currentView(long generation) {
        return generation == webGeneration && webView != null && !isDestroyed() && !isFinishing();
    }

    private void createWebView() {
        if (isDestroyed() || isFinishing()) return;
        disposeWebView(false);
        viewport.removeAllViews();
        final long generation = ++webGeneration;
        uiReady = false; recreateOnResume = false;
        try {
            CookieManager.getInstance().setAcceptCookie(false);
            WebView.setWebContentsDebuggingEnabled(BuildConfig.DEBUG);
            WebView view = new WebView(this);
            webView = view;
            displayRefresh = new DisplayRefreshController(view);
            WebSettings settings = view.getSettings();
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
            CookieManager.getInstance().setAcceptThirdPartyCookies(view, false);
            view.setBackgroundColor(Color.WHITE);
            view.setVerticalScrollBarEnabled(false);
            view.setHorizontalScrollBarEnabled(false);
            view.setOverScrollMode(View.OVER_SCROLL_NEVER);
            view.addJavascriptInterface(new ConnectionBridge(generation), "TokenMonitorAndroid");
            view.setWebViewClient(new LocalClient(generation));
            viewport.addView(view, new FrameLayout.LayoutParams(-1, -1));
            if (foreground) { view.onResume(); displayRefresh.resume(); }
            view.loadUrl(NativePolicy.LOCAL_ORIGIN + "/index.html");
            armStartupTimeout(generation);
        } catch (RuntimeException unavailable) {
            showFailure("webview_unavailable", false);
        }
    }

    private void armStartupTimeout(long generation) {
        clearStartupTimeout();
        if (!foreground || uiReady || !currentView(generation)) return;
        startupTimeout = () -> {
            startupTimeout = null;
            if (foreground && !uiReady && currentView(generation)) showFailure("startup_timeout", false);
        };
        main.postDelayed(startupTimeout, 10000);
    }

    private void clearStartupTimeout() {
        if (startupTimeout != null) main.removeCallbacks(startupTimeout);
        startupTimeout = null;
    }

    private void invalidateConnectionWork() {
        connectionEpoch.incrementAndGet();
        recoveryPermit.clear();
        dataRequests.cancelAll();
        // Test generation is owned by the main thread. Epoch invalidation is immediate
        // on the JavaScript bridge thread, before this queued cleanup can run.
        main.post(() -> { ++connectionTestGeneration; connectionTests.cancel(); });
    }

    private void disposeWebView(boolean rendererGone) {
        clearStartupTimeout();
        ++webGeneration;
        invalidateConnectionWork();
        WebView old = webView;
        webView = null; uiReady = false;
        if (displayRefresh != null) { displayRefresh.destroy(); displayRefresh = null; }
        if (old != null) {
            viewport.removeView(old);
            if (!rendererGone) {
                old.removeJavascriptInterface("TokenMonitorAndroid");
                old.stopLoading();
            }
            // A terminated renderer's WebView is never loaded or reused again.
            old.destroy();
        }
    }

    private void showFailure(String code, boolean rendererGone) {
        disposeWebView(rendererGone);
        if (isDestroyed() || isFinishing()) return;
        if (rendererGone && recoveryPolicy.allowAutomatic()) {
            if (foreground) main.post(this::createWebView);
            else recreateOnResume = true;
            return;
        }
        recreateOnResume = false;
        viewport.removeAllViews();
        LinearLayout panel = new LinearLayout(this);
        panel.setTag("inkui-recovery");
        panel.setOrientation(LinearLayout.VERTICAL);
        panel.setGravity(Gravity.CENTER);
        int pad = Math.round(12 * getResources().getDisplayMetrics().density);
        panel.setPadding(pad, pad, pad, pad);
        panel.setBackgroundColor(Color.WHITE);
        TextView title = new TextView(this);
        title.setText("界面暂时无法显示"); title.setTextSize(20); title.setTextColor(Color.BLACK);
        title.setGravity(Gravity.CENTER); title.setMaxLines(2);
        panel.addView(title, new LinearLayout.LayoutParams(-1, 0, 1));
        TextView detail = new TextView(this);
        boolean unsupported = "unsupported_webview".equals(code);
        detail.setText(unsupported ? "请更新 Android System WebView 后重试。连接配置已保留。"
                : "连接配置已保留。重试后返回原阅读位置。");
        detail.setTextSize(14); detail.setTextColor(Color.DKGRAY); detail.setGravity(Gravity.CENTER);
        detail.setMaxLines(3);
        panel.addView(detail, new LinearLayout.LayoutParams(-1, 0, 1));
        Button retry = new Button(this);
        retry.setTag("inkui-retry"); retry.setText("重试"); retry.setTextSize(18);
        retry.setAllCaps(false); retry.setMinHeight(0); retry.setMinimumHeight(0);
        retry.setOnClickListener(ignored -> createWebView());
        panel.addView(retry, new LinearLayout.LayoutParams(-1, 0, 1));
        viewport.addView(panel, new FrameLayout.LayoutParams(-1, -1));
    }

    private final class ConnectionBridge {
        private final long generation;
        ConnectionBridge(long generation) { this.generation = generation; }
        @JavascriptInterface public String getConnection() {
            if (!currentView(generation)) return "{}";
            try { return connections.publicJson().toString(); }
            catch (Exception ignored) { return "{\"storageState\":\"read_failed\"}"; }
        }
        @JavascriptInterface public String getAppInfo() {
            try {
                return new JSONObject().put("versionName", BuildConfig.VERSION_NAME)
                        .put("versionCode", BuildConfig.VERSION_CODE).toString();
            } catch (Exception ignored) { return "{}"; }
        }
        @JavascriptInterface public String saveConnection(String json) {
            if (!foreground || !currentView(generation)) return "{\"ok\":false,\"error\":\"cancelled\"}";
            try {
                String state = connections.storageState();
                if ("read_failed".equals(state) || "key_unavailable".equals(state)) {
                    JSONObject input = new JSONObject(json);
                    ConnectionStore.Config candidate = connections.candidate(json);
                    if (!Boolean.TRUE.equals(input.opt("recover")) || !recoveryPermit.consume(
                            candidate.hubUrl, candidate.hubSecret, state, candidate.revision, connectionEpoch.get()))
                        return "{\"ok\":false,\"error\":\"connection_recovery_required\"}";
                }
                String result = connections.save(json);
                if (new JSONObject(result).optBoolean("ok")) invalidateConnectionWork();
                return result;
            } catch (Exception invalid) { return "{\"ok\":false,\"error\":\"invalid_connection\"}"; }
        }
        @JavascriptInterface public String reloadConnection() {
            if (!foreground || !currentView(generation)) return "{\"ok\":false,\"error\":\"cancelled\"}";
            invalidateConnectionWork();
            return connections.reload();
        }
        @JavascriptInterface public void testConnection(String payloadJson, String requestId) {
            if (requestId == null || !requestId.matches("[A-Za-z0-9_-]{1,64}")) return;
            main.post(() -> { if (currentView(generation)) startConnectionTest(payloadJson, requestId, generation); });
        }
        @JavascriptInterface public void cancelConnectionTest() {
            recoveryPermit.clear();
            main.post(() -> { if (currentView(generation)) { ++connectionTestGeneration; connectionTests.cancel(); } });
        }
        @JavascriptInterface public void cancelDataRequest(String id) {
            if (!currentView(generation) || !validReadId(id)) return;
            long now = SystemClock.elapsedRealtime();
            synchronized (cancelledReads) {
                cancelledReads.entrySet().removeIf(entry -> now - entry.getValue() >= 30000);
                if (cancelledReads.size() >= 256 && !cancelledReads.containsKey(id)) {
                    // Excess cancellation traffic fails closed for current operations.
                    invalidateConnectionWork(); cancelledReads.clear();
                }
                cancelledReads.put(id, now);
            }
            dataRequests.cancel(id);
        }
        @JavascriptInterface public String getDisplayCapabilities() {
            DisplayRefreshController controller = displayRefresh;
            return currentView(generation) && controller != null ? controller.capabilities() : "{}";
        }
        @JavascriptInterface public String getDeviceStatus() { return deviceStatus.snapshotJson(); }
        @JavascriptInterface public void setFullscreen(boolean enabled) {
            main.post(() -> { if (currentView(generation)) fullscreen.setEnabled(enabled); });
        }
        @JavascriptInterface public void requestDisplayRefresh(String reason) {
            main.post(() -> { if (currentView(generation) && displayRefresh != null) displayRefresh.request(reason); });
        }
        @JavascriptInterface public void uiReady() {
            main.post(() -> {
                if (!currentView(generation)) return;
                uiReady = true; clearStartupTimeout();
                deviceStatus.pageReady(); dispatchVisibility();
                displayRefresh.request("content");
            });
        }
        @JavascriptInterface public void uiFailed(String code) {
            if (!"unsupported_webview".equals(code) && !"bootstrap_error".equals(code) && !"module_load_error".equals(code)) return;
            main.post(() -> { if (currentView(generation)) showFailure(code, false); });
        }
    }

    private final class LocalClient extends WebViewClient {
        private final long generation;
        LocalClient(long generation) { this.generation = generation; }
        @Override public void onPageStarted(WebView view, String url, Bitmap favicon) {
            if (currentView(generation)) deviceStatus.pageLoading();
        }
        @Override public boolean shouldOverrideUrlLoading(WebView view, WebResourceRequest request) { return true; }
        @Override public boolean shouldOverrideUrlLoading(WebView view, String url) { return true; }
        @Override public WebResourceResponse shouldInterceptRequest(WebView view, WebResourceRequest request) {
            if (!currentView(generation)) return error(409, "connection_changed");
            if (!NativePolicy.isLocalRequest(request.getUrl().toString())) return error(403, "request_blocked");
            if (!"GET".equals(request.getMethod())) return error(405, "method_not_allowed");
            String path = request.getUrl().getEncodedPath();
            if (NativePolicy.ROUTES.contains(path)) {
                String id = null;
                for (Map.Entry<String, String> header : request.getRequestHeaders().entrySet())
                    if ("X-InkUI-Request-Id".equalsIgnoreCase(header.getKey())) id = header.getValue();
                return proxy(path, id, generation);
            }
            if (!NativePolicy.ASSETS.contains(path)) return error(404, "not_found");
            try {
                String mime = path.endsWith(".css") ? "text/css" : path.endsWith(".html") ? "text/html" : "application/javascript";
                return response(200, mime, getAssets().open(path.substring(1)));
            } catch (Exception ignored) { return error(503, "ui_unavailable"); }
        }
        @Override public void onPageFinished(WebView view, String url) {
            if (currentView(generation)) dispatchVisibility();
        }
        @Override public void onReceivedError(WebView view, WebResourceRequest request, WebResourceError failure) {
            if (request.isForMainFrame() && currentView(generation)) showFailure("page_load_failed", false);
        }
        @Override public void onReceivedHttpError(WebView view, WebResourceRequest request, WebResourceResponse failure) {
            if (request.isForMainFrame() && currentView(generation)) showFailure("page_load_failed", false);
        }
        @Override public boolean onRenderProcessGone(WebView view, RenderProcessGoneDetail detail) {
            if (view == webView && currentView(generation)) showFailure("renderer_gone", true);
            // Stale instances have already been removed and destroyed by disposeWebView.
            return true;
        }
    }

    private static boolean validReadId(String id) { return id != null && id.matches("read-[A-Za-z0-9_-]{1,58}"); }
    private WebResourceResponse proxy(String path, String id, long generation) {
        if (!validReadId(id)) return error(400, "invalid_request");
        ConnectionStore.Config config = connections.snapshot();
        long epoch = connectionEpoch.get();
        if (!foreground) return error(503, "app_background");
        if (config.hubUrl.isEmpty()) return error(503, "hub_unconfigured");
        HubHttpClient.Result result = dataRequests.execute(id, config.hubUrl, config.hubSecret, path,
                () -> foreground && currentView(generation) && connectionEpoch.get() == epoch
                        && connections.isCurrent(config) && !cancelledReads.containsKey(id));
        if (!currentView(generation) || connectionEpoch.get() != epoch || !connections.isCurrent(config))
            return error(409, "connection_changed");
        if (!foreground) return error(503, "app_background");
        if (result.error == null) return response(200, "application/json", new ByteArrayInputStream(result.body));
        if ("redirect_blocked".equals(result.error)) return error(502, "upstream_redirect_blocked");
        if ("invalid_response".equals(result.error)) return error(502, "invalid_response");
        return error(503, "hub_" + result.error);
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

    private void startConnectionTest(String input, String requestId, long renderer) {
        if (isDestroyed()) return;
        long generation = ++connectionTestGeneration;
        long epoch = connectionEpoch.get();
        recoveryPermit.clear(); connectionTests.cancel();
        String origin = "";
        try {
            if (input == null || input.length() > 24576) throw new IllegalArgumentException();
            JSONObject data = new JSONObject(input);
            if (!"hub".equals(data.opt("source")) || !(data.opt("hubUrl") instanceof String)) throw new IllegalArgumentException();
            origin = NativePolicy.normalizeOrigin((String) data.opt("hubUrl"));
            ConnectionStore.Config candidate = connections.candidate(input);
            String state = connections.storageState();
            boolean recovering = Boolean.TRUE.equals(data.opt("recover"))
                    && ("read_failed".equals(state) || "key_unavailable".equals(state));
            if (!foreground) return;
            connectionTests.start(requestId, "hub", origin, candidate.hubSecret,
                    () -> foreground && currentView(renderer) && epoch == connectionEpoch.get() && connections.isCurrent(candidate),
                    result -> main.post(() -> {
                        if (generation != connectionTestGeneration || !foreground || !currentView(renderer) || epoch != connectionEpoch.get()) return;
                        String failure = connections.isCurrent(candidate) ? result.error : "cancelled";
                        if (failure == null && recovering && state.equals(connections.storageState()))
                            recoveryPermit.issue(candidate.hubUrl, candidate.hubSecret, state, candidate.revision, epoch);
                        dispatchConnectionTest(new ConnectionTestController.Result(result.id, result.source,
                                result.origin, failure, result.latencyMs));
                    }));
        } catch (Exception invalid) {
            if (foreground) dispatchConnectionTest(new ConnectionTestController.Result(requestId, "hub", origin, "invalid_connection", 0));
        }
    }

    private void dispatchConnectionTest(ConnectionTestController.Result result) {
        if (!foreground || webView == null) return;
        try {
            JSONObject detail = new JSONObject().put("id", result.id).put("ok", result.ok)
                    .put("source", result.source).put("origin", result.origin).put("latencyMs", result.latencyMs);
            if (result.error != null) detail.put("error", result.error);
            webView.evaluateJavascript("window.dispatchEvent(new CustomEvent('tokenmonitor:connection-test',{detail:" + detail + "}))", null);
        } catch (org.json.JSONException ignored) { }
    }

    private static WebResourceResponse error(int status, String code) { return json(status, "{\"error\":\"" + code + "\"}"); }
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
        if (webView != null) webView.evaluateJavascript("window.dispatchEvent(new CustomEvent('tokenmonitor:visibility',{detail:{hidden:" + (!foreground) + "}}))", null);
    }
    private void dispatchDeviceStatus(String json) {
        if (foreground && webView != null) webView.evaluateJavascript("window.dispatchEvent(new CustomEvent('tokenmonitor:device-status',{detail:" + json + "}))", null);
    }
    @Override protected void onResume() {
        super.onResume(); foreground = true;
        fullscreen.resume(); deviceStatus.start();
        if (recreateOnResume) createWebView();
        else if (webView != null) {
            webView.onResume(); displayRefresh.resume(); dispatchVisibility(); armStartupTimeout(webGeneration);
        }
    }
    @Override protected void onPause() {
        foreground = false; invalidateConnectionWork(); dispatchVisibility(); clearStartupTimeout();
        if (deviceStatus != null) deviceStatus.stop();
        if (fullscreen != null) fullscreen.pause();
        if (displayRefresh != null) displayRefresh.pause();
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
    // API 33+ uses the framework OnBackInvokedDispatcher registered in onCreate.
    // Retain this callback only for API 29-32 without adding AndroidX to the runtime.
    @android.annotation.SuppressLint("GestureBackNavigation")
    @Override public void onBackPressed() { handleBack(); }
    private void handleBack() {
        if (webView == null) { moveTaskToBack(true); return; }
        final long generation = webGeneration;
        webView.evaluateJavascript("Boolean(window.tokenMonitorBack && window.tokenMonitorBack())", handled -> {
            if (currentView(generation) && !"true".equals(handled)) moveTaskToBack(true);
        });
    }
    @Override protected void onDestroy() {
        foreground = false;
        disposeWebView(false);
        connectionTests.destroy(); dataRequests.close();
        deviceStatus.stop(); fullscreen.destroy();
        super.onDestroy();
    }
}
