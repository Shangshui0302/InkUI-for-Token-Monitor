package com.codexmonitor.mobile;

import android.app.Activity;
import android.app.AlertDialog;
import android.content.Context;
import android.content.ComponentName;
import android.content.Intent;
import android.content.ServiceConnection;
import android.graphics.Color;
import android.graphics.Canvas;
import android.graphics.Paint;
import android.graphics.Path;
import android.graphics.RectF;
import android.graphics.Typeface;
import android.graphics.drawable.GradientDrawable;
import android.net.Uri;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.os.Build;
import android.os.IBinder;
import android.view.WindowManager;
import android.view.View;
import android.view.ViewGroup;
import android.view.Gravity;
import android.view.inputmethod.InputMethodManager;
import android.webkit.JavascriptInterface;
import android.webkit.WebResourceRequest;
import android.webkit.WebResourceResponse;
import android.webkit.WebSettings;
import android.webkit.WebView;
import android.webkit.WebViewClient;
import android.widget.Button;
import android.widget.EditText;
import android.widget.ImageView;
import android.widget.LinearLayout;
import android.widget.ScrollView;
import android.widget.TextView;
import android.widget.Toast;

import org.json.JSONArray;
import org.json.JSONObject;

import java.io.ByteArrayInputStream;
import java.net.Inet6Address;
import java.net.InetAddress;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;

public final class MainActivity extends Activity {
    private static final String PREFS = "node-settings";
    private static final String PREF_URL = "node-url";
    private static final String PREF_HIDE_STATUS = "hide-status-bar";
    private static final String PREF_HIDE_NAVIGATION = "hide-navigation-bar";
    private static final String PREF_THEME = "native-theme";
    private static final long MAIN_DOCUMENT_TIMEOUT_MS = 30000L;

    private WebView webView;
    private final Handler mainHandler = new Handler(Looper.getMainLooper());
    private Runnable mainDocumentTimeout;
    private String activeMainFrameUrl;
    private long mainNavigationId;
    private boolean mainNavigationPending;
    private NodeService nodeService;
    private boolean nodeBound;
    private LinearLayout nodePanel;
    private LinearLayout dashboardBody;
    private boolean dashboardVisible;
    private TextView nodeStatusText;
    private TextView nodeIdentityText;
    private String nodePublicKey = "";
    private int dashboardTab;
    private TextView nodeCodeText;
    private TextView nearbyHeading;
    private TextView pairedHeading;
    private LinearLayout nearbyList;
    private LinearLayout pairedList;
    private EditText remotePairingCode;
    private EditText manualAddress;
    private final Runnable refreshNodePage = new Runnable() {
        @Override
        public void run() {
            if (nodePanel != null) {
                if (dashboardVisible && dashboardBody != null) renderDashboard(dashboardBody);
                else renderNodeStatus();
                mainHandler.postDelayed(this, 2000L);
            }
        }
    };
    private final ServiceConnection nodeConnection = new ServiceConnection() {
        @Override
        public void onServiceConnected(ComponentName name, IBinder binder) {
            nodeService = ((NodeService.LocalBinder) binder).service();
            nodeService.setChangeListener(MainActivity.this::refreshVisiblePage);
            if (dashboardVisible && dashboardBody != null) renderDashboard(dashboardBody);
            else renderNodeStatus();
        }

        @Override
        public void onServiceDisconnected(ComponentName name) {
            nodeService = null;
            if (dashboardVisible && dashboardBody != null) renderDashboard(dashboardBody);
            else renderNodeStatus();
        }
    };

    private void refreshVisiblePage() {
        if (Looper.myLooper() != Looper.getMainLooper()) {
            runOnUiThread(this::refreshVisiblePage);
            return;
        }
        if (dashboardVisible && dashboardBody != null) renderDashboard(dashboardBody);
        else renderNodeStatus();
    }

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        startNodeService();
        showDashboardPage();
    }

    @Override
    public void onBackPressed() {
        if (webView != null && webView.canGoBack()) {
            webView.goBack();
        } else if (webView != null) {
            showNodePage();
        } else if (nodePanel != null) {
            showDashboardPage();
        } else {
            super.onBackPressed();
        }
    }

    @Override
    protected void onDestroy() {
        mainHandler.removeCallbacks(refreshNodePage);
        if (nodeService != null) nodeService.setChangeListener(null);
        if (nodeBound) {
            unbindService(nodeConnection);
            nodeBound = false;
        }
        destroyWebView();
        super.onDestroy();
    }

    @Override
    public void onWindowFocusChanged(boolean hasFocus) {
        super.onWindowFocusChanged(hasFocus);
        if (hasFocus && webView != null) {
            applySystemBars();
        }
    }

    private void showSetup(String currentUrl) {
        showSetup(currentUrl, null, false);
    }

    private void showSetup(String currentUrl, String error, boolean recovery) {
        showSystemBars();
        destroyWebView();

        int padding = dp(20);
        LinearLayout panel = new LinearLayout(this);
        panel.setOrientation(LinearLayout.VERTICAL);
        panel.setPadding(padding, padding, padding, padding);
        panel.setBackgroundColor(Color.WHITE);

        ScrollView scrollView = new ScrollView(this);
        scrollView.setFillViewport(true);
        scrollView.addView(panel, new ScrollView.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT));

        TextView title = new TextView(this);
        title.setText("Codex Monitor");
        title.setTextSize(22);
        title.setTextColor(Color.rgb(28, 35, 42));
        panel.addView(title, new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT));

        TextView help = new TextView(this);
        help.setText("输入任意 Codex Monitor 桌面节点的网页地址。地址只保存在这台设备上。");
        help.setTextSize(15);
        help.setTextColor(Color.rgb(75, 84, 93));
        LinearLayout.LayoutParams helpParams = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        helpParams.topMargin = dp(12);
        panel.addView(help, helpParams);

        if (error != null && !error.isEmpty()) {
            TextView errorText = new TextView(this);
            errorText.setText(error);
            errorText.setTextSize(15);
            errorText.setTextColor(Color.rgb(165, 45, 45));
            LinearLayout.LayoutParams errorParams = new LinearLayout.LayoutParams(
                    ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
            errorParams.topMargin = dp(16);
            panel.addView(errorText, errorParams);
        }

        EditText input = new EditText(this);
        input.setSingleLine(true);
        input.setHint("https://… or http://192.168.x.x:port");
        input.setInputType(android.text.InputType.TYPE_CLASS_TEXT
                | android.text.InputType.TYPE_TEXT_VARIATION_URI);
        input.setText(currentUrl == null ? "" : currentUrl);
        LinearLayout.LayoutParams inputParams = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        inputParams.topMargin = dp(18);
        panel.addView(input, inputParams);

        Button connect = new Button(this);
        connect.setText("保存并连接");
        LinearLayout.LayoutParams buttonParams = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        buttonParams.topMargin = dp(12);
        panel.addView(connect, buttonParams);

        connect.setOnClickListener(view -> connectToNode(input));

        if (recovery) {
            Button retry = new Button(this);
            retry.setText("重试当前地址");
            LinearLayout.LayoutParams retryParams = new LinearLayout.LayoutParams(
                    ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.WRAP_CONTENT);
            retryParams.topMargin = dp(8);
            panel.addView(retry, retryParams);
            retry.setOnClickListener(view -> connectToNode(input));

            Button exit = new Button(this);
            exit.setText("退出");
            LinearLayout.LayoutParams exitParams = new LinearLayout.LayoutParams(
                    ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.WRAP_CONTENT);
            exitParams.topMargin = dp(4);
            panel.addView(exit, exitParams);
            exit.setOnClickListener(view -> finish());
        }

        getWindow().setSoftInputMode(WindowManager.LayoutParams.SOFT_INPUT_ADJUST_RESIZE);
        setContentView(scrollView);
    }

    private void connectToNode(EditText input) {
        String url = input.getText().toString().trim();
        if (!isAllowedNodeUrl(url)) {
            input.setError("请输入 HTTP(S) 地址；HTTP 只能指向本地网络。");
            return;
        }
        getPreferences(MODE_PRIVATE).edit().putString(PREF_URL, url).apply();
        ((InputMethodManager) getSystemService(Context.INPUT_METHOD_SERVICE))
                .hideSoftInputFromWindow(input.getWindowToken(), 0);
        showWebView(url);
    }

    private void startNodeService() {
        Intent intent = new Intent(this, NodeService.class);
        if (Build.VERSION.SDK_INT >= 26) startForegroundService(intent);
        else startService(intent);
        nodeBound = bindService(intent, nodeConnection, Context.BIND_AUTO_CREATE);
    }

    private String nativeTheme() {
        return getSharedPreferences(PREFS, MODE_PRIVATE).getString(PREF_THEME, "light");
    }

    private boolean darkTheme() { return "dark".equals(nativeTheme()); }
    private boolean einkTheme() { return "eink".equals(nativeTheme()); }
    private int pageColor() { return darkTheme() ? Color.rgb(18, 20, 22) : Color.rgb(247, 247, 242); }
    private int cardColor() { return darkTheme() ? Color.rgb(35, 39, 43) : Color.WHITE; }
    private int textColor() { return darkTheme() ? Color.rgb(241, 241, 236) : Color.BLACK; }
    private int secondaryColor() { return darkTheme() ? Color.rgb(184, 190, 196) : Color.DKGRAY; }
    private int borderColor() { return darkTheme() ? Color.rgb(105, 111, 117) : Color.BLACK; }

    private void showDashboardPage() {
        showSystemBars();
        destroyWebView();
        int padding = dp(14);
        LinearLayout page = new LinearLayout(this);
        page.setOrientation(LinearLayout.VERTICAL);
        page.setPadding(padding, padding, padding, padding);
        page.setBackgroundColor(pageColor());

        LinearLayout header = new LinearLayout(this);
        header.setGravity(Gravity.CENTER_VERTICAL);
        ImageView cat = new ImageView(this);
        cat.setImageResource(getResources().getIdentifier("app_icon_master", "drawable", getPackageName()));
        cat.setScaleType(ImageView.ScaleType.CENTER_INSIDE);
        header.addView(cat, new LinearLayout.LayoutParams(dp(42), dp(42)));
        LinearLayout titles = new LinearLayout(this); titles.setOrientation(LinearLayout.VERTICAL);
        LinearLayout.LayoutParams titleParams = new LinearLayout.LayoutParams(0,
                ViewGroup.LayoutParams.WRAP_CONTENT, 1f); titleParams.leftMargin = dp(9);
        header.addView(titles, titleParams);
        TextView title = plainText("Codex Monitor", 20, textColor());
        title.setTypeface(Typeface.DEFAULT, Typeface.BOLD); titles.addView(title);
        titles.addView(plainText("本机持有的网格监控视图", 11, secondaryColor()));
        View settings = iconButton(LineIconButton.GEAR, "节点与配对");
        settings.setOnClickListener(view -> showNodePage()); header.addView(settings);
        page.addView(header);

        String[] labels = {"概览", "配额", "动态", "会话"};
        LinearLayout tabs = new LinearLayout(this); tabs.setOrientation(LinearLayout.HORIZONTAL);
        for (int i = 0; i < labels.length; i++) {
            final int selected = i;
            Button tab = actionButton(labels[i], i == dashboardTab);
            tab.setOnClickListener(view -> { dashboardTab = selected; showDashboardPage(); });
            LinearLayout.LayoutParams p = new LinearLayout.LayoutParams(0, dp(40), 1f);
            if (i > 0) p.leftMargin = dp(5); tabs.addView(tab, p);
        }
        LinearLayout.LayoutParams tabsParams = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        tabsParams.topMargin = dp(10); page.addView(tabs, tabsParams);

        ScrollView scroll = new ScrollView(this); scroll.setFillViewport(true);
        LinearLayout body = new LinearLayout(this); body.setOrientation(LinearLayout.VERTICAL);
        dashboardBody = body; dashboardVisible = true; nodeStatusText = null;
        scroll.addView(body, new ScrollView.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT));
        LinearLayout.LayoutParams scrollParams = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, 0, 1f); scrollParams.topMargin = dp(8);
        page.addView(scroll, scrollParams);
        renderDashboard(body);
        setContentView(page); nodePanel = page;
        mainHandler.removeCallbacks(refreshNodePage); mainHandler.postDelayed(refreshNodePage, 2000L);
    }

    private void renderDashboard(LinearLayout body) {
        body.removeAllViews();
        NodeService.NodeSnapshot state = nodeService == null ? null : nodeService.snapshot();
        List<NodeService.MonitorState> monitors = state == null
                ? Collections.emptyList() : state.monitors;
        if (monitors.isEmpty()) {
            LinearLayout empty = card(body, 0);
            TextView heading = plainText("等待已配对节点的监控数据", 17, textColor());
            heading.setTypeface(Typeface.DEFAULT, Typeface.BOLD); empty.addView(heading);
            empty.addView(nodeText(null, "状态快照通过 TCP 8787 的已认证连接同步；不需要打开网页端口。", 13, 8));
            return;
        }
        if (dashboardTab == 0) renderOverview(body, monitors, state);
        else if (dashboardTab == 1) renderQuota(body, monitors);
        else if (dashboardTab == 2) renderActivity(body, monitors);
        else renderSessions(body, monitors);
    }

    private void renderOverview(LinearLayout body, List<NodeService.MonitorState> monitors,
            NodeService.NodeSnapshot node) {
        int sessions = 0, running = 0;
        for (NodeService.MonitorState host : monitors) {
            sessions += host.sessions.length();
            for (int i = 0; i < host.sessions.length(); i++) {
                if (isActiveStatus(host.sessions.optJSONObject(i).optString("status"))) running++;
            }
        }
        LinearLayout totals = card(body, 0);
        totals.addView(metricRow("网格节点", String.valueOf(monitors.size()),
                "已配对在线 " + onlineCount(node.paired)));
        totals.addView(metricRow("会话", String.valueOf(sessions), "活动中 " + running));
        for (NodeService.MonitorState host : monitors) {
            LinearLayout card = card(body, 8);
            card.addView(hostHeading(host));
            String age = ageText(host.capturedAt);
            card.addView(nodeText(null, "采集器 " + localizedCollector(host.collectorStatus)
                    + " · " + age + (host.os.isEmpty() ? "" : " · " + host.os), 12, 6));
            if (!host.error.isEmpty()) card.addView(nodeText(null, "异常：" + host.error, 12, 5));
        }
    }

    private void renderQuota(LinearLayout body, List<NodeService.MonitorState> monitors) {
        for (NodeService.MonitorState host : monitors) {
            LinearLayout hostCard = card(body, body.getChildCount() == 0 ? 0 : 8);
            hostCard.addView(hostHeading(host));
            if (host.account == null) { hostCard.addView(nodeText(null, "暂无账户配额数据", 12, 7)); continue; }
            JSONArray limits = host.account.optJSONArray("rateLimits");
            if (limits == null || limits.length() == 0) {
                hostCard.addView(nodeText(null, "暂无速率限制窗口", 12, 7)); continue;
            }
            for (int i = 0; i < limits.length(); i++) {
                JSONObject bucket = limits.optJSONObject(i); if (bucket == null) continue;
                String name = bucket.isNull("limitName") ? "" : bucket.optString("limitName", "");
                if (name.isEmpty()) name = bucket.optString("limitId", "配额");
                JSONObject primary = bucket.optJSONObject("primary");
                String used = percent(primary == null ? null : primary.opt("usedPercent"));
                hostCard.addView(metricRow(name, used, resetText(primary)));
            }
        }
    }

    private void renderActivity(LinearLayout body, List<NodeService.MonitorState> monitors) {
        for (NodeService.MonitorState host : monitors) {
            LinearLayout hostCard = card(body, body.getChildCount() == 0 ? 0 : 8);
            hostCard.addView(hostHeading(host));
            hostCard.addView(nodeText(null, "状态快照更新于 " + ageText(host.capturedAt), 12, 6));
            int shown = 0;
            for (int i = 0; i < host.sessions.length() && shown < 8; i++) {
                JSONObject session = host.sessions.optJSONObject(i); if (session == null) continue;
                String summary = session.optString("lastOutputSummary", "");
                if (summary.isEmpty()) continue;
                hostCard.addView(nodeText(null, "• " + session.optString("title", "未命名会话")
                        + "：" + summary, 12, 7)); shown++;
            }
            if (shown == 0) hostCard.addView(nodeText(null, "暂无最近输出摘要", 12, 7));
        }
    }

    private void renderSessions(LinearLayout body, List<NodeService.MonitorState> monitors) {
        for (NodeService.MonitorState host : monitors) {
            LinearLayout hostCard = card(body, body.getChildCount() == 0 ? 0 : 8);
            hostCard.addView(hostHeading(host));
            for (int i = 0; i < host.sessions.length(); i++) {
                JSONObject session = host.sessions.optJSONObject(i); if (session == null) continue;
                String title = session.optString("title", session.optString("threadId", "未命名会话"));
                String status = session.optString("status", "unknown");
                hostCard.addView(metricRow(title, localizedSession(status),
                        ageText(session.optLong("updatedAt", host.capturedAt))));
            }
            if (host.sessions.length() == 0) hostCard.addView(nodeText(null, "暂无会话", 12, 7));
        }
    }

    private TextView hostHeading(NodeService.MonitorState host) {
        TextView view = plainText(host.alias, 16, textColor());
        view.setTypeface(Typeface.DEFAULT, Typeface.BOLD); return view;
    }

    private LinearLayout metricRow(String label, String value, String detail) {
        LinearLayout row = new LinearLayout(this); row.setGravity(Gravity.CENTER_VERTICAL);
        row.setPadding(0, dp(7), 0, dp(5));
        LinearLayout left = new LinearLayout(this); left.setOrientation(LinearLayout.VERTICAL);
        TextView name = plainText(label, 13, textColor()); left.addView(name);
        if (detail != null && !detail.isEmpty()) left.addView(plainText(detail, 10, secondaryColor()));
        row.addView(left, new LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f));
        TextView number = plainText(value, 16, textColor()); number.setTypeface(Typeface.MONOSPACE, Typeface.BOLD);
        row.addView(number); return row;
    }

    private int onlineCount(List<NodeService.PeerInfo> peers) { int n = 0; for (NodeService.PeerInfo p : peers) if (p.online) n++; return n; }
    private boolean isActiveStatus(String value) { return "running".equals(value) || "in_progress".equals(value) || "active".equals(value); }
    private String localizedCollector(String value) { if ("ready".equals(value)) return "正常"; if ("auth_required".equals(value)) return "需要登录"; if ("degraded".equals(value)) return "降级"; return "不可用"; }
    private String localizedSession(String value) { return isActiveStatus(value) ? "进行中" : ("completed".equals(value) ? "已完成" : value); }
    private String percent(Object value) { return value == null || value == JSONObject.NULL ? "—" : Math.round(((Number) value).doubleValue()) + "%"; }
    private String resetText(JSONObject window) {
        if (window == null) return "";
        long reset = window.optLong("resetsAt", 0L);
        if (reset == 0) return "";
        long remain = reset - System.currentTimeMillis() / 1000L;
        if (remain <= 0) return "即将重置";
        return remain < 3600 ? remain / 60 + " 分钟后重置" : remain / 3600 + " 小时后重置";
    }
    private String ageText(long seconds) { long delta = System.currentTimeMillis() / 1000L - seconds; if (delta < 0) delta = 0; if (delta < 60) return delta + " 秒前"; if (delta < 3600) return delta / 60 + " 分钟前"; return delta / 3600 + " 小时前"; }

    private void showNodePage() {
        showSystemBars();
        destroyWebView();
        dashboardVisible = false; dashboardBody = null;
        int padding = dp(14);
        LinearLayout content = new LinearLayout(this);
        content.setOrientation(LinearLayout.VERTICAL);
        content.setPadding(padding, padding, padding, padding);
        content.setBackgroundColor(pageColor());

        ScrollView scroll = new ScrollView(this);
        scroll.setFillViewport(true);
        scroll.addView(content, new ScrollView.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT));

        LinearLayout header = new LinearLayout(this);
        header.setOrientation(LinearLayout.HORIZONTAL);
        header.setGravity(Gravity.CENTER_VERTICAL);
        ImageView cat = new ImageView(this);
        cat.setImageResource(getResources().getIdentifier(
                "app_icon_master", "drawable", getPackageName()));
        cat.setScaleType(ImageView.ScaleType.CENTER_INSIDE);
        header.addView(cat, new LinearLayout.LayoutParams(dp(46), dp(46)));
        LinearLayout titleBlock = new LinearLayout(this);
        titleBlock.setOrientation(LinearLayout.VERTICAL);
        LinearLayout.LayoutParams titleBlockParams = new LinearLayout.LayoutParams(
                0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f);
        titleBlockParams.leftMargin = dp(10);
        header.addView(titleBlock, titleBlockParams);
        TextView title = plainText("监控节点", 21, textColor());
        title.setTypeface(Typeface.DEFAULT, Typeface.BOLD);
        titleBlock.addView(title);
        TextView subtitle = plainText("Poke3 · Android 独立节点", 12, secondaryColor());
        titleBlock.addView(subtitle);
        Button openMonitor = actionButton("监控面板", false);
        openMonitor.setOnClickListener(view -> showDashboardPage());
        header.addView(openMonitor);
        content.addView(header);

        LinearLayout themeRow = new LinearLayout(this);
        themeRow.setGravity(Gravity.CENTER_VERTICAL);
        themeRow.addView(plainText("显示模式", 12, secondaryColor()),
                new LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f));
        addThemeButton(themeRow, "浅色", "light");
        if (!einkTheme()) addThemeButton(themeRow, "深色", "dark");
        addThemeButton(themeRow, "电子墨水", "eink");
        LinearLayout.LayoutParams themeParams = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        themeParams.topMargin = dp(7); content.addView(themeRow, themeParams);

        LinearLayout statusCard = card(content, 10);
        LinearLayout statusRow = new LinearLayout(this);
        statusRow.setOrientation(LinearLayout.HORIZONTAL);
        statusRow.setGravity(Gravity.CENTER_VERTICAL);
        TextView statusTitle = plainText("本机节点", 15, textColor());
        statusTitle.setTypeface(Typeface.DEFAULT, Typeface.BOLD);
        statusRow.addView(statusTitle, new LinearLayout.LayoutParams(
                0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f));
        Button rename = actionButton("改名", false);
        LinearLayout.LayoutParams renameParams = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        renameParams.rightMargin = dp(6);
        statusRow.addView(rename, renameParams);
        rename.setOnClickListener(view -> showRenameDialog());
        nodeStatusText = plainText("启动中", 13, Color.WHITE);
        nodeStatusText.setGravity(Gravity.CENTER);
        nodeStatusText.setPadding(dp(10), dp(3), dp(10), dp(3));
        nodeStatusText.setBackground(roundedBackground(Color.BLACK, Color.BLACK, 99));
        statusRow.addView(nodeStatusText);
        statusCard.addView(statusRow);
        nodeIdentityText = plainText("身份载入中…", 12, secondaryColor());
        nodeIdentityText.setTypeface(Typeface.MONOSPACE);
        LinearLayout.LayoutParams identityParams = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        identityParams.topMargin = dp(5);
        statusCard.addView(nodeIdentityText, identityParams);
        nodePublicKey = "P-256 公钥载入中…";

        LinearLayout codeCard = card(content, 8);
        LinearLayout codeRow = new LinearLayout(this);
        codeRow.setOrientation(LinearLayout.HORIZONTAL);
        codeRow.setGravity(Gravity.CENTER_VERTICAL);
        LinearLayout codeBlock = new LinearLayout(this);
        codeBlock.setOrientation(LinearLayout.VERTICAL);
        codeBlock.addView(plainText("本机一次性配对码", 12, secondaryColor()));
        nodeCodeText = plainText("载入中…", 22, textColor());
        nodeCodeText.setTypeface(Typeface.MONOSPACE, Typeface.BOLD);
        nodeCodeText.setLetterSpacing(0.08f);
        codeBlock.addView(nodeCodeText);
        codeRow.addView(codeBlock, new LinearLayout.LayoutParams(
                0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f));
        View refreshCode = iconButton(LineIconButton.REFRESH, "刷新一次性配对码");
        refreshCode.setOnClickListener(view -> {
            if (nodeService != null) {
                String refreshed = nodeService.refreshPairingCode();
                nodeCodeText.setText(refreshed);
                Toast.makeText(this, "旧配对码已立即失效", Toast.LENGTH_SHORT).show();
            }
        });
        codeRow.addView(refreshCode);
        codeCard.addView(codeRow);
        TextView codeHint = plainText("首次配对成功后自动失效；已信任节点重连无需配对码。", 11, secondaryColor());
        LinearLayout.LayoutParams codeHintParams = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        codeHintParams.topMargin = dp(3);
        codeCard.addView(codeHint, codeHintParams);

        LinearLayout pairRow = new LinearLayout(this);
        pairRow.setOrientation(LinearLayout.HORIZONTAL);
        pairRow.setGravity(Gravity.CENTER_VERTICAL);
        remotePairingCode = new EditText(this);
        remotePairingCode.setSingleLine(true);
        remotePairingCode.setHint("输入对方 8 位配对码");
        remotePairingCode.setTextSize(14);
        remotePairingCode.setInputType(android.text.InputType.TYPE_CLASS_TEXT
                | android.text.InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS);
        remotePairingCode.setFilters(new android.text.InputFilter[]{new android.text.InputFilter.LengthFilter(8)});
        pairRow.addView(remotePairingCode, new LinearLayout.LayoutParams(
                0, dp(44), 1f));
        Button advanced = actionButton("高级设置", false);
        LinearLayout.LayoutParams advancedParams = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        advancedParams.leftMargin = dp(8);
        pairRow.addView(advanced, advancedParams);
        advanced.setOnClickListener(view -> showAdvancedNodeSettings());
        LinearLayout.LayoutParams pairRowParams = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        pairRowParams.topMargin = dp(6);
        content.addView(pairRow, pairRowParams);

        LinearLayout peerColumns = new LinearLayout(this);
        boolean landscape = getResources().getConfiguration().orientation
                == android.content.res.Configuration.ORIENTATION_LANDSCAPE;
        peerColumns.setOrientation(landscape ? LinearLayout.HORIZONTAL : LinearLayout.VERTICAL);
        LinearLayout nearbyCard = card(null, 0);
        nearbyHeading = sectionTitle("附近节点");
        nearbyCard.addView(nearbyHeading);
        nearbyList = new LinearLayout(this);
        nearbyList.setOrientation(LinearLayout.VERTICAL);
        nearbyCard.addView(nearbyList);
        LinearLayout pairedCard = card(null, 0);
        pairedHeading = sectionTitle("已配对节点");
        pairedCard.addView(pairedHeading);
        pairedList = new LinearLayout(this);
        pairedList.setOrientation(LinearLayout.VERTICAL);
        pairedCard.addView(pairedList);
        LinearLayout.LayoutParams nearbyParams = new LinearLayout.LayoutParams(
                landscape ? 0 : ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT, landscape ? 1f : 0f);
        LinearLayout.LayoutParams pairedParams = new LinearLayout.LayoutParams(
                landscape ? 0 : ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT, landscape ? 1f : 0f);
        if (landscape) pairedParams.leftMargin = dp(8); else pairedParams.topMargin = dp(8);
        peerColumns.addView(nearbyCard, nearbyParams);
        peerColumns.addView(pairedCard, pairedParams);
        LinearLayout.LayoutParams columnsParams = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        columnsParams.topMargin = dp(8);
        content.addView(peerColumns, columnsParams);

        setContentView(scroll);
        nodePanel = content;
        renderNodeStatus();
        mainHandler.removeCallbacks(refreshNodePage);
        mainHandler.postDelayed(refreshNodePage, 2000L);
    }

    private TextView plainText(String text, float size, int color) {
        TextView view = new TextView(this);
        view.setText(text);
        view.setTextSize(size);
        view.setTextColor(color);
        view.setIncludeFontPadding(false);
        return view;
    }

    private void addThemeButton(LinearLayout row, String label, String value) {
        Button button = actionButton(label, value.equals(nativeTheme()));
        LinearLayout.LayoutParams params = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.WRAP_CONTENT, dp(36)); params.leftMargin = dp(5);
        row.addView(button, params);
        button.setOnClickListener(view -> {
            getSharedPreferences(PREFS, MODE_PRIVATE).edit().putString(PREF_THEME, value).apply();
            showNodePage();
        });
    }

    private TextView sectionTitle(String text) {
        TextView view = plainText(text, 15, textColor());
        view.setTypeface(Typeface.DEFAULT, Typeface.BOLD);
        return view;
    }

    private LinearLayout card(LinearLayout parent, int topMarginDp) {
        LinearLayout view = new LinearLayout(this);
        view.setOrientation(LinearLayout.VERTICAL);
        view.setPadding(dp(10), dp(9), dp(10), dp(9));
        view.setBackground(roundedBackground(cardColor(), borderColor(), 10));
        if (parent != null) {
            LinearLayout.LayoutParams params = new LinearLayout.LayoutParams(
                    ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
            params.topMargin = dp(topMarginDp);
            parent.addView(view, params);
        }
        return view;
    }

    private GradientDrawable roundedBackground(int fill, int stroke, int radiusDp) {
        GradientDrawable drawable = new GradientDrawable();
        drawable.setColor(fill);
        drawable.setStroke(dp(1), stroke);
        drawable.setCornerRadius(dp(radiusDp));
        return drawable;
    }

    private Button actionButton(String text, boolean primary) {
        Button button = new Button(this);
        button.setText(text);
        button.setTextSize(12);
        button.setAllCaps(false);
        button.setTextColor(primary ? (darkTheme() ? Color.BLACK : Color.WHITE) : textColor());
        button.setGravity(Gravity.CENTER);
        button.setMinHeight(0);
        button.setMinWidth(0);
        button.setMinimumHeight(0);
        button.setMinimumWidth(0);
        button.setPadding(dp(12), dp(7), dp(12), dp(7));
        button.setBackground(roundedBackground(primary ? textColor() : cardColor(), borderColor(), 8));
        if (Build.VERSION.SDK_INT >= 21) button.setStateListAnimator(null);
        return button;
    }

    private View iconButton(int icon, String description) {
        LineIconButton button = new LineIconButton(this, icon);
        button.setContentDescription(description);
        button.setFocusable(true);
        button.setClickable(true);
        button.setBackgroundColor(Color.TRANSPARENT);
        button.setPadding(0, 0, 0, 0);
        button.setMinimumWidth(0);
        button.setMinimumHeight(0);
        button.setLayoutParams(new LinearLayout.LayoutParams(dp(52), dp(52)));
        return button;
    }

    private final class LineIconButton extends View {
        static final int REFRESH = 1;
        static final int GEAR = 2;
        private final int icon;
        private final Paint paint = new Paint(Paint.ANTI_ALIAS_FLAG);

        LineIconButton(Context context, int icon) {
            super(context);
            this.icon = icon;
            setLayerType(View.LAYER_TYPE_SOFTWARE, null);
        }

        @Override
        protected void onDraw(Canvas canvas) {
            super.onDraw(canvas);
            float stroke = dp(einkTheme() ? 2 : 1.4f);
            float inset = stroke;
            paint.setStyle(Paint.Style.STROKE);
            paint.setStrokeWidth(stroke);
            paint.setStrokeCap(Paint.Cap.ROUND);
            paint.setStrokeJoin(Paint.Join.ROUND);
            paint.setColor(borderColor());
            canvas.drawRoundRect(new RectF(inset, inset, getWidth() - inset,
                    getHeight() - inset), dp(12), dp(12), paint);
            if (icon == REFRESH) drawRefresh(canvas); else drawGear(canvas);
        }

        private void drawRefresh(Canvas canvas) {
            float cx = getWidth() / 2f, cy = getHeight() / 2f;
            float radius = dp(11);
            paint.setStrokeWidth(dp(einkTheme() ? 2.2f : 1.8f));
            canvas.drawArc(new RectF(cx - radius, cy - radius, cx + radius, cy + radius),
                    -65, 285, false, paint);
            Path arrow = new Path();
            arrow.moveTo(cx - radius - dp(1), cy - dp(8));
            arrow.lineTo(cx - radius - dp(1), cy - dp(14));
            arrow.lineTo(cx - radius + dp(5), cy - dp(11));
            canvas.drawPath(arrow, paint);
        }

        private void drawGear(Canvas canvas) {
            float cx = getWidth() / 2f, cy = getHeight() / 2f;
            float inner = dp(10), outer = dp(14);
            Path gear = new Path();
            for (int i = 0; i < 32; i++) {
                double angle = -Math.PI / 2 + i * Math.PI / 16;
                float radius = (i % 4 == 1 || i % 4 == 2) ? outer : inner;
                float x = cx + (float) Math.cos(angle) * radius;
                float y = cy + (float) Math.sin(angle) * radius;
                if (i == 0) gear.moveTo(x, y); else gear.lineTo(x, y);
            }
            gear.close();
            paint.setStrokeWidth(dp(einkTheme() ? 2.2f : 1.8f));
            canvas.drawPath(gear, paint);
            canvas.drawCircle(cx, cy, dp(4), paint);
        }
    }

    private TextView nodeText(LinearLayout parent, String text, float size, int topMarginDp) {
        TextView view = new TextView(this);
        view.setText(text);
        view.setTextSize(size);
        view.setTextColor(secondaryColor());
        LinearLayout.LayoutParams params = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        params.topMargin = dp(topMarginDp);
        if (parent != null) parent.addView(view, params);
        return view;
    }

    private void renderNodeStatus() {
        if (Looper.myLooper() != Looper.getMainLooper()) {
            runOnUiThread(this::renderNodeStatus);
            return;
        }
        if (nodePanel == null) return;
        if (nodeService == null) {
            nodeStatusText.setText("连接中");
            return;
        }
        NodeService.NodeSnapshot state = nodeService.snapshot();
        nodeStatusText.setText(state.ready ? "在线" : localizedNodeStatus(state.status));
        nodeStatusText.setBackground(roundedBackground(
                state.ready ? Color.BLACK : Color.rgb(90, 90, 90), Color.BLACK, 99));
        nodeIdentityText.setText(state.alias + "  ·  " + state.hostId + "  ·  附近 " + state.nearby.size());
        nodePublicKey = "P-256 公钥（SPKI DER hex）\n" + state.publicKey;
        nodeCodeText.setText(state.pairingCode);
        List<NodeService.PeerInfo> unpaired = new ArrayList<>();
        for (NodeService.PeerInfo peer : state.nearby) {
            if (!peer.paired) unpaired.add(peer);
        }
        nearbyHeading.setText("附近节点  " + unpaired.size());
        pairedHeading.setText("已配对节点  " + state.paired.size());
        renderPeerList(nearbyList, unpaired, true);
        renderPeerList(pairedList, state.paired, false);
    }

    private String localizedNodeStatus(String status) {
        if (status == null || status.isEmpty()) return "连接中";
        if (status.startsWith("Waiting for Wi-Fi")) return "等待 Wi-Fi";
        if (status.startsWith("Identity initialization failed")) return "身份异常";
        if (status.startsWith("Starting")) return "启动中";
        return status;
    }

    private void showRenameDialog() {
        if (nodeService == null) return;
        EditText input = new EditText(this);
        input.setSingleLine(true);
        input.setText(nodeService.snapshot().alias);
        input.setSelectAllOnFocus(true);
        input.setFilters(new android.text.InputFilter[]{new android.text.InputFilter.LengthFilter(64)});
        AlertDialog dialog = new AlertDialog.Builder(this)
                .setTitle("修改节点昵称")
                .setView(input)
                .setNegativeButton("取消", null)
                .setPositiveButton("保存", null)
                .create();
        dialog.setOnShowListener(ignored -> dialog.getButton(AlertDialog.BUTTON_POSITIVE)
                .setOnClickListener(view -> {
                    try {
                        nodeService.setAlias(input.getText().toString());
                        dialog.dismiss();
                        renderNodeStatus();
                        showToast("昵称已保存，正在重新连接");
                    } catch (IllegalArgumentException error) {
                        input.setError(error.getMessage());
                    }
                }));
        dialog.show();
    }

    private void renderPeerList(LinearLayout parent, List<NodeService.PeerInfo> peers, boolean allowPair) {
        parent.removeAllViews();
        if (peers.isEmpty()) {
            TextView empty = plainText(allowPair ? "暂未发现新节点" : "还没有已配对节点", 12, Color.DKGRAY);
            LinearLayout.LayoutParams emptyParams = new LinearLayout.LayoutParams(
                    ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
            emptyParams.topMargin = dp(8);
            parent.addView(empty, emptyParams);
            return;
        }
        for (NodeService.PeerInfo peer : peers) {
            LinearLayout row = new LinearLayout(this);
            row.setOrientation(LinearLayout.VERTICAL);
            row.setPadding(dp(8), dp(7), dp(8), dp(7));
            row.setBackground(roundedBackground(Color.rgb(247, 247, 242), Color.rgb(140, 140, 140), 7));
            LinearLayout.LayoutParams rowParams = new LinearLayout.LayoutParams(
                    ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
            rowParams.topMargin = dp(6);
            parent.addView(row, rowParams);
            LinearLayout nameRow = new LinearLayout(this);
            nameRow.setOrientation(LinearLayout.HORIZONTAL);
            nameRow.setGravity(Gravity.CENTER_VERTICAL);
            TextView name = plainText(peer.alias, 14, Color.BLACK);
            name.setTypeface(Typeface.DEFAULT, Typeface.BOLD);
            nameRow.addView(name, new LinearLayout.LayoutParams(
                    0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f));
            TextView peerState = plainText(peer.paired
                    ? (peer.online ? "在线" : "离线") : "待配对", 11, Color.DKGRAY);
            nameRow.addView(peerState);
            row.addView(nameRow);
            TextView detail = plainText(peer.hostId
                    + (peer.address.isEmpty() ? "" : "  ·  " + peer.address), 10, Color.DKGRAY);
            detail.setTypeface(Typeface.MONOSPACE);
            LinearLayout.LayoutParams detailParams = new LinearLayout.LayoutParams(
                    ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
            detailParams.topMargin = dp(3);
            row.addView(detail, detailParams);
            if (allowPair && !peer.paired) {
                Button pair = actionButton("配对", true);
                LinearLayout.LayoutParams pairParams = new LinearLayout.LayoutParams(
                        ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.WRAP_CONTENT);
                pairParams.topMargin = dp(5);
                pairParams.gravity = Gravity.RIGHT;
                row.addView(pair, pairParams);
                pair.setOnClickListener(view -> {
                    if (nodeService == null) return;
                    try {
                        nodeService.pairWith(peer.hostId, remotePairingCode.getText().toString());
                        showToast("已发起配对");
                    } catch (IllegalArgumentException error) {
                        remotePairingCode.setError(error.getMessage());
                    }
                });
            }
        }
    }

    private void openSavedMonitor() {
        String saved = getPreferences(MODE_PRIVATE).getString(PREF_URL, "");
        if (saved.isEmpty()) showSetup(null); else showWebView(saved);
    }

    private void showAdvancedNodeSettings() {
        LinearLayout panel = new LinearLayout(this);
        panel.setOrientation(LinearLayout.VERTICAL);
        panel.setPadding(dp(4), 0, dp(4), 0);
        TextView explanation = plainText(
                "仅在自动发现不可用时填写地址。配对仍使用上方输入的对方当前配对码。", 13, Color.DKGRAY);
        panel.addView(explanation);
        manualAddress = new EditText(this);
        manualAddress.setSingleLine(true);
        manualAddress.setHint("192.168.0.20:8787");
        manualAddress.setInputType(android.text.InputType.TYPE_CLASS_TEXT
                | android.text.InputType.TYPE_TEXT_VARIATION_URI);
        LinearLayout.LayoutParams addressParams = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        addressParams.topMargin = dp(8);
        panel.addView(manualAddress, addressParams);
        if (!nodePublicKey.isEmpty()) {
            TextView publicKey = plainText(nodePublicKey, 11, Color.DKGRAY);
            publicKey.setTextIsSelectable(true);
            LinearLayout.LayoutParams keyParams = new LinearLayout.LayoutParams(
                    ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
            keyParams.topMargin = dp(10);
            panel.addView(publicKey, keyParams);
        }
        new AlertDialog.Builder(this)
                .setTitle("高级设置")
                .setView(panel)
                .setPositiveButton("按地址配对", (dialog, which) -> {
                    if (nodeService == null) {
                        showToast("节点服务尚未连接");
                        return;
                    }
                    try {
                        nodeService.pairWithAddress(manualAddress.getText().toString().trim(),
                                remotePairingCode.getText().toString());
                        showToast("已发起配对");
                    } catch (IllegalArgumentException | IllegalStateException error) {
                        showToast(error.getMessage());
                    }
                })
                .setNeutralButton("修改网页地址", (dialog, which) -> showSetup(
                        getPreferences(MODE_PRIVATE).getString(PREF_URL, "")))
                .setNegativeButton("关闭", null)
                .show();
    }

    private void showToast(String message) {
        Toast.makeText(this, message, Toast.LENGTH_SHORT).show();
    }

    private void showRootMenu() {
        final String currentUrl = activeMainFrameUrl != null
                ? activeMainFrameUrl
                : getPreferences(MODE_PRIVATE).getString(PREF_URL, "");
        new AlertDialog.Builder(this)
                .setTitle("Codex Monitor")
                .setItems(new String[]{"打开网页监控", "修改网页地址", "重新加载", "退出"}, (dialog, which) -> {
                    if (which == 0) {
                        openSavedMonitor();
                    } else if (which == 1) {
                        showSetup(currentUrl);
                    } else if (which == 2) {
                        if (webView != null) {
                            scheduleMainDocumentTimeout(webView, currentUrl);
                            webView.reload();
                        } else if (!currentUrl.isEmpty()) {
                            showWebView(currentUrl);
                        }
                    } else if (which == 3) {
                        finish();
                    }
                })
                .setNegativeButton("取消", (dialog, which) -> dialog.dismiss())
                .show();
    }

    private void destroyWebView() {
        WebView oldWebView = webView;
        webView = null;
        nodePanel = null;
        mainHandler.removeCallbacks(refreshNodePage);
        activeMainFrameUrl = null;
        mainNavigationId++;
        clearMainDocumentTimeout();
        if (oldWebView != null) {
            oldWebView.stopLoading();
            oldWebView.destroy();
        }
    }

    private void connectFailure(WebView view, String url, String reason) {
        if (view != webView || url == null || !isActiveMainFrameUrl(url)) {
            return;
        }
        showSetup(url, reason, true);
    }

    private boolean isActiveMainFrameUrl(String url) {
        if (activeMainFrameUrl == null) {
            return true;
        }
        return activeMainFrameUrl.equals(url);
    }

    private void scheduleMainDocumentTimeout(WebView view, String url) {
        clearMainDocumentTimeout();
        startMainDocumentTimeout(view, url);
    }

    private void startMainDocumentTimeout(WebView view, String url) {
        if (mainNavigationPending) {
            activeMainFrameUrl = url;
            return;
        }
        final long navigationId = ++mainNavigationId;
        mainNavigationPending = true;
        activeMainFrameUrl = url;
        mainDocumentTimeout = () -> {
            String failedUrl = activeMainFrameUrl;
            if (view == webView && navigationId == mainNavigationId
                    && failedUrl != null) {
                connectFailure(view, failedUrl,
                        "页面在 30 秒内未完成加载。请检查地址和网络后重试或修改地址。");
            }
        };
        mainHandler.postDelayed(mainDocumentTimeout, MAIN_DOCUMENT_TIMEOUT_MS);
    }

    private void clearMainDocumentTimeout() {
        if (mainDocumentTimeout != null) {
            mainHandler.removeCallbacks(mainDocumentTimeout);
            mainDocumentTimeout = null;
        }
        mainNavigationPending = false;
    }

    private void showWebView(String url) {
        destroyWebView();
        webView = new WebView(this);
        final WebView thisWebView = webView;
        webView.setBackgroundColor(Color.WHITE);
        WebSettings settings = webView.getSettings();
        settings.setJavaScriptEnabled(true);
        settings.setDomStorageEnabled(true);
        settings.setAllowFileAccess(false);
        settings.setAllowContentAccess(false);
        settings.setMixedContentMode(WebSettings.MIXED_CONTENT_NEVER_ALLOW);
        settings.setUserAgentString(settings.getUserAgentString() + " CodexMonitorAndroid/0.1.0");
        webView.addJavascriptInterface(new AndroidChromeBridge(thisWebView), "CodexMonitorAndroid");
        webView.setWebViewClient(new WebViewClient() {
            @Override
            public boolean shouldOverrideUrlLoading(WebView view, WebResourceRequest request) {
                Uri requestedUri = request.getUrl();
                if (view == webView && request.isForMainFrame() && mayLoad(requestedUri)) {
                    scheduleMainDocumentTimeout(view, requestedUri.toString());
                }
                return !mayLoad(requestedUri);
            }

            @Override
            public boolean shouldOverrideUrlLoading(WebView view, String requestedUrl) {
                if (view == webView && mayLoad(Uri.parse(requestedUrl))) {
                    scheduleMainDocumentTimeout(view, requestedUrl);
                }
                return !mayLoad(Uri.parse(requestedUrl));
            }

            @Override
            public WebResourceResponse shouldInterceptRequest(WebView view, WebResourceRequest request) {
                if (!mayLoad(request.getUrl())) {
                    return blockedResponse();
                }
                return super.shouldInterceptRequest(view, request);
            }

            @Override
            public void onPageStarted(WebView view, String url, android.graphics.Bitmap favicon) {
                super.onPageStarted(view, url, favicon);
                if (view == webView && (view.getUrl() == null || url.equals(view.getUrl()))) {
                    if (mainNavigationPending) {
                        activeMainFrameUrl = url;
                    } else {
                        startMainDocumentTimeout(view, url);
                    }
                }
            }

            @Override
            public void onPageFinished(WebView view, String url) {
                super.onPageFinished(view, url);
                if (view == webView && isActiveMainFrameUrl(url)
                        && (view.getUrl() == null || url.equals(view.getUrl()))) {
                    activeMainFrameUrl = url;
                    clearMainDocumentTimeout();
                }
            }

            @Override
            public void onPageCommitVisible(WebView view, String url) {
                super.onPageCommitVisible(view, url);
                if (view == webView && isActiveMainFrameUrl(url)
                        && (view.getUrl() == null || url.equals(view.getUrl()))) {
                    activeMainFrameUrl = url;
                    clearMainDocumentTimeout();
                }
            }

            @Override
            public void onReceivedError(WebView view, WebResourceRequest request,
                    android.webkit.WebResourceError error) {
                super.onReceivedError(view, request, error);
                if (request.isForMainFrame()) {
                    connectFailure(view, request.getUrl().toString(),
                            "页面加载失败：" + error.getDescription());
                }
            }

            @Override
            public void onReceivedHttpError(WebView view, WebResourceRequest request,
                    WebResourceResponse response) {
                super.onReceivedHttpError(view, request, response);
                if (request.isForMainFrame() && response.getStatusCode() >= 400) {
                    connectFailure(view, request.getUrl().toString(),
                            "服务器返回 HTTP " + response.getStatusCode()
                                    + " " + response.getReasonPhrase() + "。请检查地址或稍后重试。");
                }
            }
        });
        setContentView(webView);
        applySystemBars();
        startMainDocumentTimeout(webView, url);
        webView.loadUrl(url);
    }

    private void applySystemBars() {
        int flags = View.SYSTEM_UI_FLAG_LAYOUT_STABLE;
        if (getPreferences(MODE_PRIVATE).getBoolean(PREF_HIDE_STATUS, true)) {
            flags |= View.SYSTEM_UI_FLAG_FULLSCREEN | View.SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN;
        }
        if (getPreferences(MODE_PRIVATE).getBoolean(PREF_HIDE_NAVIGATION, true)) {
            flags |= View.SYSTEM_UI_FLAG_HIDE_NAVIGATION
                    | View.SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION
                    | View.SYSTEM_UI_FLAG_IMMERSIVE_STICKY;
        }
        getWindow().getDecorView().setSystemUiVisibility(flags);
    }

    private void showSystemBars() {
        getWindow().getDecorView().setSystemUiVisibility(View.SYSTEM_UI_FLAG_LAYOUT_STABLE);
    }

    private final class AndroidChromeBridge {
        private final WebView owner;

        AndroidChromeBridge(WebView owner) {
            this.owner = owner;
        }

        @JavascriptInterface
        public boolean getHideStatusBar() {
            return getPreferences(MODE_PRIVATE).getBoolean(PREF_HIDE_STATUS, true);
        }

        @JavascriptInterface
        public boolean getHideNavigationBar() {
            return getPreferences(MODE_PRIVATE).getBoolean(PREF_HIDE_NAVIGATION, true);
        }

        @JavascriptInterface
        public void setHideStatusBar(boolean hide) {
            getPreferences(MODE_PRIVATE).edit().putBoolean(PREF_HIDE_STATUS, hide).apply();
            runOnUiThread(MainActivity.this::applySystemBars);
        }

        @JavascriptInterface
        public void setHideNavigationBar(boolean hide) {
            getPreferences(MODE_PRIVATE).edit().putBoolean(PREF_HIDE_NAVIGATION, hide).apply();
            runOnUiThread(MainActivity.this::applySystemBars);
        }

        @JavascriptInterface
        public void showNodeAddressSettings() {
            runOnUiThread(() -> {
                if (webView == owner) {
                    showSetup(getPreferences(MODE_PRIVATE).getString(PREF_URL, ""));
                }
            });
        }
    }

    private boolean mayLoad(Uri uri) {
        String scheme = uri.getScheme();
        if ("https".equalsIgnoreCase(scheme)) {
            return true;
        }
        return "http".equalsIgnoreCase(scheme) && isLocalHttpHost(uri.getHost());
    }

    private static WebResourceResponse blockedResponse() {
        return new WebResourceResponse(
                "text/plain", "UTF-8", 403, "Blocked by local HTTP policy",
                Collections.<String, String>emptyMap(),
                new ByteArrayInputStream(new byte[0]));
    }

    private static boolean isAllowedNodeUrl(String value) {
        try {
            Uri uri = Uri.parse(value);
            String scheme = uri.getScheme();
            if (uri.getHost() == null || uri.getUserInfo() != null
                    || !("https".equalsIgnoreCase(scheme) || "http".equalsIgnoreCase(scheme))) {
                return false;
            }
            return "https".equalsIgnoreCase(scheme) || isLocalHttpHost(uri.getHost());
        } catch (RuntimeException ex) {
            return false;
        }
    }

    private static boolean isLocalHttpHost(String host) {
        if (host == null) {
            return false;
        }
        String value = host.toLowerCase(java.util.Locale.ROOT);
        if ("localhost".equals(value) || value.endsWith(".localhost") || value.endsWith(".local")) {
            return true;
        }
        if (value.indexOf(':') >= 0) {
            try {
                InetAddress address = InetAddress.getByName(value);
                return address.isLoopbackAddress() || address.isLinkLocalAddress()
                        || (address instanceof Inet6Address
                        && (address.isSiteLocalAddress() || isUniqueLocalAddress((Inet6Address) address)));
            } catch (Exception ex) {
                return false;
            }
        }
        String[] octets = value.split("\\.");
        if (octets.length != 4) {
            return false;
        }
        int[] ip = new int[4];
        for (int i = 0; i < octets.length; i++) {
            try {
                ip[i] = Integer.parseInt(octets[i]);
            } catch (NumberFormatException ex) {
                return false;
            }
            if (ip[i] < 0 || ip[i] > 255) {
                return false;
            }
        }
        return ip[0] == 10
                || ip[0] == 127
                || ip[0] == 192 && ip[1] == 168
                || ip[0] == 172 && ip[1] >= 16 && ip[1] <= 31
                || ip[0] == 169 && ip[1] == 254;
    }

    private static boolean isUniqueLocalAddress(Inet6Address address) {
        int first = address.getAddress()[0] & 0xff;
        return first == 0xfc || first == 0xfd;
    }

    private int dp(int value) {
        return Math.round(value * getResources().getDisplayMetrics().density);
    }

    private float dp(float value) {
        return value * getResources().getDisplayMetrics().density;
    }
}
