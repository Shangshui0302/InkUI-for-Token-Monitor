package dev.tokenmonitor.eink;

import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.content.IntentFilter;
import android.net.ConnectivityManager;
import android.net.Network;
import android.net.NetworkCapabilities;
import android.net.NetworkRequest;
import android.net.LinkProperties;
import android.os.BatteryManager;
import android.os.Build;
import android.os.Handler;
import android.os.Looper;
import java.time.ZoneId;
import java.util.HashMap;
import java.util.Map;
import java.util.function.Consumer;

/** Main-thread owner of foreground-only listeners; the JS bridge reads one volatile immutable snapshot. */
final class DeviceStatusController {
    private final Context context;
    private final ConnectivityManager connectivity;
    private final Handler handler = new Handler(Looper.getMainLooper());
    private final Consumer<String> listener;
    private final Map<Network, DeviceStatus.Link> links = new HashMap<>();
    private final Map<Network, LinkProperties> routes = new HashMap<>();
    private volatile DeviceStatus snapshot;
    private DeviceStatus lastSent;
    private boolean active;
    private boolean pageReady;
    private boolean fullscreen;
    private boolean receiverRegistered;
    private boolean networkReadable;
    private Integer batteryPercent;
    private Boolean charging;
    private ConnectivityManager.NetworkCallback networkCallback;
    private long generation;
    private long networkRevision;
    private final Runnable minute = () -> { if (active) { publish(); scheduleMinute(); } };
    private final BroadcastReceiver receiver = new BroadcastReceiver() {
        @Override public void onReceive(Context ignored, Intent intent) {
            if (!active) return;
            if (Intent.ACTION_BATTERY_CHANGED.equals(intent.getAction())) readBattery(intent);
            publish();
            scheduleMinute();
        }
    };

    DeviceStatusController(Context context, boolean fullscreen, Consumer<String> listener) {
        this.context = context;
        this.fullscreen = fullscreen;
        this.listener = listener;
        this.connectivity = context.getSystemService(ConnectivityManager.class);
        // A one-shot sticky read also makes the first synchronous bridge call useful.
        readBattery(context.registerReceiver(null, new IntentFilter(Intent.ACTION_BATTERY_CHANGED)));
        readNetworks();
        publish();
    }

    String snapshotJson() { return snapshot.toJson(); }

    void setFullscreen(boolean enabled) { fullscreen = enabled; publish(); }

    void pageLoading() { pageReady = false; lastSent = null; }
    void pageReady() { pageReady = true; publish(); }

    void start() {
        if (active) return;
        active = true;
        final long ticket = ++generation;
        IntentFilter filter = new IntentFilter(Intent.ACTION_BATTERY_CHANGED);
        filter.addAction(Intent.ACTION_TIME_CHANGED);
        filter.addAction(Intent.ACTION_TIMEZONE_CHANGED);
        // System-only broadcasts need no runtime permission; receive battery changes from system UIDs too.
        Intent battery = Build.VERSION.SDK_INT >= 33
                ? context.registerReceiver(receiver, filter, Context.RECEIVER_EXPORTED)
                : context.registerReceiver(receiver, filter);
        receiverRegistered = true;
        readBattery(battery);
        if (connectivity != null) {
            ConnectivityManager.NetworkCallback callback = new ConnectivityManager.NetworkCallback() {
                private boolean current() { return active && ticket == generation; }
                @Override public void onAvailable(Network network) {
                    if (!current()) return;
                    // Capabilities follow onAvailable. Never query synchronous network APIs here.
                    if (!links.containsKey(network)) {
                        links.put(network, DeviceStatus.Link.UNKNOWN); networkRevision++;
                    }
                }
                @Override public void onCapabilitiesChanged(Network network, NetworkCapabilities capabilities) {
                    if (!current()) return;
                    networkReadable = true;
                    links.put(network, describe(capabilities));
                    publish();
                }
                @Override public void onLinkPropertiesChanged(Network network, LinkProperties properties) {
                    if (!current()) return;
                    LinkProperties previous = routes.put(network, properties);
                    if (!properties.equals(previous)) { networkRevision++; publish(); }
                }
                @Override public void onLost(Network network) {
                    if (!current()) return;
                    networkReadable = true;
                    if (links.remove(network) != null) networkRevision++;
                    routes.remove(network);
                    publish();
                }
            };
            try {
                // No INTERNET/VALIDATED requirement: a LAN-only Wi-Fi connection is still a device link.
                NetworkRequest.Builder builder = new NetworkRequest.Builder();
                if (android.os.Build.VERSION.SDK_INT >= 30) builder.clearCapabilities();
                else builder.removeCapability(NetworkCapabilities.NET_CAPABILITY_TRUSTED)
                        .removeCapability(NetworkCapabilities.NET_CAPABILITY_NOT_VPN);
                NetworkRequest request = builder.addCapability(NetworkCapabilities.NET_CAPABILITY_NOT_RESTRICTED).build();
                connectivity.registerNetworkCallback(request, callback, handler);
                networkCallback = callback;
            } catch (RuntimeException ignored) { networkCallback = null; }
        }
        readNetworks();
        if (networkCallback == null) networkReadable = false;
        publish();
        scheduleMinute();
    }

    void stop() {
        active = false;
        generation++;
        handler.removeCallbacks(minute);
        if (receiverRegistered) { context.unregisterReceiver(receiver); receiverRegistered = false; }
        if (networkCallback != null) {
            try { connectivity.unregisterNetworkCallback(networkCallback); }
            catch (RuntimeException ignored) { /* An OEM may already have removed the callback. */ }
            networkCallback = null;
        }
    }

    private void readBattery(Intent intent) {
        if (intent == null) { batteryPercent = null; charging = null; return; }
        batteryPercent = DeviceStatus.batteryPercent(intent.getIntExtra(BatteryManager.EXTRA_LEVEL, -1),
                intent.getIntExtra(BatteryManager.EXTRA_SCALE, -1));
        int status = intent.getIntExtra(BatteryManager.EXTRA_STATUS, BatteryManager.BATTERY_STATUS_UNKNOWN);
        if (status == BatteryManager.BATTERY_STATUS_CHARGING || status == BatteryManager.BATTERY_STATUS_FULL) charging = true;
        else if (status == BatteryManager.BATTERY_STATUS_DISCHARGING || status == BatteryManager.BATTERY_STATUS_NOT_CHARGING) charging = false;
        else charging = null;
    }

    private void readNetworks() {
        Map<Network, LinkProperties> previous = new HashMap<>(routes);
        links.clear();
        routes.clear();
        networkReadable = false;
        if (connectivity == null) return;
        try {
            for (Network network : connectivity.getAllNetworks()) {
                NetworkCapabilities capabilities = connectivity.getNetworkCapabilities(network);
                if (capabilities == null || capabilities.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_RESTRICTED)) {
                    links.put(network, describe(capabilities));
                    LinkProperties properties = connectivity.getLinkProperties(network);
                    if (properties != null) routes.put(network, properties);
                }
            }
            if (!routes.equals(previous)) networkRevision++;
            networkReadable = true;
        } catch (RuntimeException ignored) { links.clear(); routes.clear(); }
    }

    private static DeviceStatus.Link describe(NetworkCapabilities capabilities) {
        if (capabilities == null) return DeviceStatus.Link.UNKNOWN;
        String transport = capabilities.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET) ? "ethernet"
                : capabilities.hasTransport(NetworkCapabilities.TRANSPORT_WIFI) ? "wifi"
                : capabilities.hasTransport(NetworkCapabilities.TRANSPORT_CELLULAR) ? "cellular" : "other";
        return new DeviceStatus.Link(transport, capabilities.hasCapability(NetworkCapabilities.NET_CAPABILITY_VALIDATED));
    }

    private void publish() {
        DeviceStatus value = new DeviceStatus(fullscreen, System.currentTimeMillis(), ZoneId.systemDefault(),
                batteryPercent, charging, DeviceStatus.Link.select(links.values(), networkReadable), networkRevision);
        snapshot = value;
        if (active && pageReady && !value.equals(lastSent)) { lastSent = value; listener.accept(value.toJson()); }
    }

    private void scheduleMinute() {
        handler.removeCallbacks(minute);
        if (active) handler.postDelayed(minute, DeviceStatus.millisToNextMinute(System.currentTimeMillis()));
    }
}
