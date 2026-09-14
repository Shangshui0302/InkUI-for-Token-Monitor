package com.codexmonitor.mobile;

import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.app.Service;
import android.content.Context;
import android.content.Intent;
import android.net.wifi.WifiManager;
import android.os.Binder;
import android.os.Build;
import android.os.Handler;
import android.os.IBinder;
import android.os.Looper;
import android.util.Log;

import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;

import java.io.ByteArrayOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.net.DatagramPacket;
import java.net.Inet4Address;
import java.net.InetAddress;
import java.net.InetSocketAddress;
import java.net.InterfaceAddress;
import java.net.MulticastSocket;
import java.net.NetworkInterface;
import java.net.ServerSocket;
import java.net.Socket;
import java.net.SocketAddress;
import java.net.SocketException;
import java.nio.charset.StandardCharsets;
import java.security.SecureRandom;
import java.util.ArrayList;
import java.util.Collections;
import java.util.Enumeration;
import java.util.List;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.ScheduledFuture;
import java.util.concurrent.Semaphore;
import java.util.concurrent.ThreadFactory;
import java.util.concurrent.TimeUnit;

/** Persistent Android peer node. Its listeners are independent of Activity and WebView lifetimes. */
public final class NodeService extends Service {
    private static final String TAG = "CodexMonitorNode";
    private static final String PREFS = "node-settings";
    private static final String PREF_CODE = "node-pairing-code";
    private static final String PREF_TRUSTED = "node-trusted-peers";
    private static final String PREF_SNAPSHOTS = "node-monitor-snapshots";
    private static final String PREF_ALIAS = "node-alias";
    private static final int DISCOVERY_PORT = 37991;
    private static final int PEER_PORT = 8787;
    private static final String MULTICAST_ADDRESS = "239.255.77.77";
    private static final String SERVICE_NAME = "codex-monitor";
    private static final String NETWORK_ID = "codex-monitor-lan";
    private static final String PEER_SYNC_CAPABILITY = "peer-sync-v1";
    private static final String P256_PEER_SYNC_CAPABILITY = "peer-sync-p256-v1";
    private static final String STATE_SNAPSHOT_CAPABILITY = "state-snapshot-v1";
    private static final int MAX_UDP_BYTES = 8192;
    private static final int MAX_WIRE_BYTES = 64 * 1024;
    private static final int MAX_ACTIVE_CONNECTIONS = 32;
    private static final long ANNOUNCE_EVERY_SECONDS = 5L;
    private static final long ANNOUNCEMENT_TTL_SECONDS = 20L;
    private static final long PEER_TIMEOUT_SECONDS = 45L;

    private final IBinder binder = new LocalBinder();
    private final Handler mainHandler = new Handler(Looper.getMainLooper());
    private final Object stateLock = new Object();
    private final Map<String, SeenPeer> nearby = new ConcurrentHashMap<>();
    private final Map<String, TrustedPeer> trusted = new ConcurrentHashMap<>();
    private final Map<String, MonitorState> monitorStates = new ConcurrentHashMap<>();
    private final Map<String, Integer> activeConnections = new ConcurrentHashMap<>();
    private final Map<String, Boolean> connecting = new ConcurrentHashMap<>();
    private final java.util.Set<Socket> activeSockets = ConcurrentHashMap.newKeySet();
    private final Semaphore connectionSlots = new Semaphore(MAX_ACTIVE_CONNECTIONS);
    private final SecureRandom random = new SecureRandom();
    private final ExecutorService io = Executors.newCachedThreadPool(namedThreads("node-io"));
    private final ScheduledExecutorService scheduler = Executors.newScheduledThreadPool(
            2, namedThreads("node-timer"));
    private WifiManager.MulticastLock multicastLock;
    private volatile ChangeListener changeListener;
    private volatile NodeIdentity identity;
    private volatile PairingCode pairingCode;
    private volatile ServerSocket serverSocket;
    private volatile MulticastSocket discoverySocket;
    private volatile NetworkInterface wifiInterface;
    private volatile boolean ready;
    private volatile boolean stopping;
    private volatile String startupError = "Starting network listeners…";
    private volatile String lastError = "";
    private volatile String nodeAlias = "Codex Monitor Android";
    private long sequence;

    public final class LocalBinder extends Binder {
        public NodeService service() {
            return NodeService.this;
        }
    }

    public interface ChangeListener {
        void onNodeStateChanged();
    }

    public static final class PeerInfo {
        public final String hostId;
        public final String alias;
        public final String address;
        public final boolean paired;
        public final boolean online;
        public final String pairingPublicKey;

        PeerInfo(String hostId, String alias, String address, boolean paired,
                boolean online, String pairingPublicKey) {
            this.hostId = hostId;
            this.alias = alias;
            this.address = address;
            this.paired = paired;
            this.online = online;
            this.pairingPublicKey = pairingPublicKey;
        }
    }

    public static final class NodeSnapshot {
        public final String hostId;
        public final String publicKey;
        public final String pairingCode;
        public final String alias;
        public final boolean ready;
        public final String status;
        public final List<PeerInfo> nearby;
        public final List<PeerInfo> paired;
        public final List<MonitorState> monitors;

        NodeSnapshot(String hostId, String publicKey, String pairingCode, String alias,
                boolean ready, String status, List<PeerInfo> nearby, List<PeerInfo> paired,
                List<MonitorState> monitors) {
            this.hostId = hostId;
            this.publicKey = publicKey;
            this.pairingCode = pairingCode;
            this.alias = alias;
            this.ready = ready;
            this.status = status;
            this.nearby = nearby;
            this.paired = paired;
            this.monitors = monitors;
        }
    }

    public static final class MonitorState {
        public final String hostId, alias, os, collectorStatus, error;
        public final long sequence, capturedAt;
        public final JSONArray sessions;
        public final JSONObject account;

        MonitorState(JSONObject value) throws JSONException {
            hostId = value.getString("hostId"); alias = value.getString("alias");
            os = value.optString("os", ""); sequence = value.getLong("sequence");
            capturedAt = value.getLong("capturedAt");
            collectorStatus = value.getString("collectorStatus");
            error = value.isNull("error") ? "" : value.optString("error", "");
            JSONArray sourceSessions = value.optJSONArray("sessions");
            sessions = new JSONArray(sourceSessions == null ? "[]" : sourceSessions.toString());
            JSONObject sourceAccount = value.optJSONObject("account");
            account = sourceAccount == null ? null : new JSONObject(sourceAccount.toString());
        }

        JSONObject json() throws JSONException {
            JSONObject value = new JSONObject();
            value.put("hostId", hostId); value.put("alias", alias);
            if (!os.isEmpty()) value.put("os", os);
            value.put("sequence", sequence); value.put("capturedAt", capturedAt);
            value.put("collectorStatus", collectorStatus);
            value.put("error", error.isEmpty() ? JSONObject.NULL : error);
            value.put("sessions", sessions);
            value.put("account", account == null ? JSONObject.NULL : account);
            value.put("networkId", NETWORK_ID); value.put("sourceNodeId", hostId);
            return value;
        }
    }

    @Override
    public void onCreate() {
        super.onCreate();
        createNotificationChannel();
        startForeground(1, makeNotification("Starting local node…"));
        WifiManager wifi = (WifiManager) getApplicationContext().getSystemService(Context.WIFI_SERVICE);
        multicastLock = wifi == null ? null : wifi.createMulticastLock("codex-monitor-discovery");
        if (multicastLock != null) {
            multicastLock.setReferenceCounted(false);
            multicastLock.acquire();
        }
        loadTrustedPeers();
        loadMonitorStates();
        nodeAlias = getSharedPreferences(PREFS, MODE_PRIVATE)
                .getString(PREF_ALIAS, "Codex Monitor Android").trim();
        if (nodeAlias.isEmpty()) nodeAlias = "Codex Monitor Android";
        try {
            identity = NodeIdentity.loadOrCreate();
            pairingCode = new PairingCode(getSharedPreferences(PREFS, MODE_PRIVATE)
                    .getString(PREF_CODE, null));
            persistPairingCode();
            publishStatus("Starting Wi-Fi discovery and TCP listener…");
            startNetwork();
        } catch (Exception error) {
            Log.e(TAG, "Could not initialize the AndroidKeyStore node identity", error);
            startupError = "Identity initialization failed: " + safeMessage(error);
            publishStatus(startupError);
        }
    }

    @Override
    public int onStartCommand(Intent intent, int flags, int startId) {
        return START_STICKY;
    }

    @Override
    public IBinder onBind(Intent intent) {
        return binder;
    }

    @Override
    public void onDestroy() {
        stopping = true;
        ready = false;
        ServerSocket tcp = serverSocket;
        MulticastSocket udp = discoverySocket;
        if (tcp != null) {
            try { tcp.close(); } catch (IOException ignored) { }
        }
        if (udp != null) {
            udp.close();
        }
        scheduler.shutdownNow();
        io.shutdownNow();
        if (multicastLock != null && multicastLock.isHeld()) {
            multicastLock.release();
        }
        super.onDestroy();
    }

    public void setChangeListener(ChangeListener listener) {
        changeListener = listener;
    }

    public NodeSnapshot snapshot() {
        ArrayList<PeerInfo> nearbyItems = new ArrayList<>();
        long now = System.currentTimeMillis();
        for (SeenPeer peer : nearby.values()) {
            if (now - peer.seenAt <= TimeUnit.SECONDS.toMillis(ANNOUNCEMENT_TTL_SECONDS * 2)) {
                nearbyItems.add(new PeerInfo(peer.hostId, peer.alias, peer.address,
                        trusted.containsKey(peer.hostId),
                        activeConnections.getOrDefault(peer.hostId, 0) > 0,
                        peer.pairingPublicKey));
            }
        }
        Collections.sort(nearbyItems, (a, b) -> a.alias.compareToIgnoreCase(b.alias));
        ArrayList<PeerInfo> pairedItems = new ArrayList<>();
        for (TrustedPeer peer : trusted.values()) {
            SeenPeer seen = nearby.get(peer.hostId);
            String address = seen == null ? peer.lastAddress : seen.address;
            pairedItems.add(new PeerInfo(peer.hostId, peer.alias, address, true,
                    activeConnections.getOrDefault(peer.hostId, 0) > 0, peer.pairingPublicKey));
        }
        Collections.sort(pairedItems, (a, b) -> a.alias.compareToIgnoreCase(b.alias));
        ArrayList<MonitorState> monitors = new ArrayList<>(monitorStates.values());
        Collections.sort(monitors, (a, b) -> a.alias.compareToIgnoreCase(b.alias));
        NodeIdentity currentIdentity = identity;
        PairingCode currentCode = pairingCode;
        return new NodeSnapshot(
                currentIdentity == null ? "Unavailable" : currentIdentity.hostId(),
                currentIdentity == null ? "Unavailable" : currentIdentity.publicKey(),
                currentCode == null ? "Unavailable" : currentCode.current(),
                nodeAlias,
                ready,
                ready ? "Listening · Wi-Fi " + (wifiInterface == null ? "" : wifiInterface.getName())
                        + (lastError.isEmpty() ? "" : " · Last issue: " + lastError)
                        : startupError,
                nearbyItems,
                pairedItems,
                monitors);
    }

    public String refreshPairingCode() {
        PairingCode current = pairingCode;
        if (current == null) {
            return "Unavailable";
        }
        String code = current.refresh();
        persistPairingCode();
        publishStatus(ready ? "Running" : startupError);
        return code;
    }

    public void setAlias(String value) {
        String trimmed = value == null ? "" : value.trim();
        if (trimmed.isEmpty() || trimmed.length() > 64) {
            throw new IllegalArgumentException("昵称需为 1–64 个字符");
        }
        nodeAlias = trimmed;
        getSharedPreferences(PREFS, MODE_PRIVATE).edit().putString(PREF_ALIAS, trimmed).apply();
        announceSafely();
        for (Socket socket : activeSockets) closeQuietly(socket);
        publishStatus(ready ? "Running" : startupError);
    }

    public void pairWith(String hostId, String remoteCode) {
        if (!PairingCode.isValid(remoteCode)) {
            throw new IllegalArgumentException("配对码必须恰好为 8 位英文字母或数字。");
        }
        SeenPeer peer = nearby.get(hostId);
        if (peer == null) {
            throw new IllegalArgumentException("该节点已不在附近，请刷新后重试。");
        }
        if (!isP256PublicKey(peer.pairingPublicKey)) {
            throw new IllegalArgumentException("该节点尚未提供 P-256 配对公钥。");
        }
        startOutbound(peer, remoteCode);
    }

    public void pairWithAddress(String address, String remoteCode) {
        if (!PairingCode.isValid(remoteCode)) {
            throw new IllegalArgumentException("配对码必须恰好为 8 位英文字母或数字。");
        }
        String[] endpoint = parseEndpoint(address);
        if (!ready) throw new IllegalStateException("原生节点仍在启动中。");
        io.execute(() -> pairManual(endpoint[0], Integer.parseInt(endpoint[1]), remoteCode));
    }

    private void startNetwork() {
        io.execute(() -> {
            while (!stopping && !ready) {
                ServerSocket tcp = null;
                MulticastSocket udp = null;
                try {
                    NetworkInterface iface = findWifiInterface();
                    if (iface == null) {
                        throw new IOException("No active Wi-Fi interface with multicast support was found.");
                    }
                    tcp = new ServerSocket();
                    tcp.setReuseAddress(true);
                    tcp.bind(new InetSocketAddress(PEER_PORT), 32);
                    udp = new MulticastSocket(null);
                    udp.setReuseAddress(true);
                    udp.bind(new InetSocketAddress(DISCOVERY_PORT));
                    udp.setTimeToLive(1);
                    udp.setNetworkInterface(iface);
                    InetAddress group = InetAddress.getByName(MULTICAST_ADDRESS);
                    udp.joinGroup(new InetSocketAddress(group, DISCOVERY_PORT), iface);

                    wifiInterface = iface;
                    serverSocket = tcp;
                    discoverySocket = udp;
                    final ServerSocket readyTcp = tcp;
                    final MulticastSocket readyUdp = udp;
                    io.execute(() -> acceptLoop(readyTcp));
                    io.execute(() -> discoveryLoop(readyUdp));
                    ready = true;
                    startupError = "Running";
                    startForeground(1, makeNotification("Node online · TCP 8787 · UDP 37991"));
                    publishStatus("Running");
                    scheduler.scheduleAtFixedRate(this::announceSafely, 0,
                            ANNOUNCE_EVERY_SECONDS, TimeUnit.SECONDS);
                    scheduler.scheduleAtFixedRate(this::expireOldDiscoveries, 10,
                            10, TimeUnit.SECONDS);
                    Log.i(TAG, "Native node ready on " + iface.getName());
                    return;
                } catch (Exception error) {
                    Log.w(TAG, "Node listeners are not ready; retrying", error);
                    startupError = "Waiting for Wi-Fi: " + safeMessage(error);
                    publishStatus(startupError);
                    closeQuietly(tcp);
                    if (udp != null) {
                        udp.close();
                    }
                    try { Thread.sleep(5000L); }
                    catch (InterruptedException interrupted) { Thread.currentThread().interrupt(); return; }
                }
            }
        });
    }

    private void acceptLoop(ServerSocket tcp) {
        while (!stopping && !tcp.isClosed()) {
            try {
                Socket socket = tcp.accept();
                if (!connectionSlots.tryAcquire()) {
                    closeQuietly(socket);
                    continue;
                }
                io.execute(() -> handleInbound(socket));
            } catch (SocketException closed) {
                if (!stopping) Log.w(TAG, "TCP accept stopped", closed);
                return;
            } catch (IOException error) {
                if (!stopping) Log.w(TAG, "TCP accept failed", error);
            }
        }
    }

    private void discoveryLoop(MulticastSocket udp) {
        byte[] buffer = new byte[MAX_UDP_BYTES + 1];
        while (!stopping && !udp.isClosed()) {
            try {
                DatagramPacket packet = new DatagramPacket(buffer, buffer.length);
                udp.receive(packet);
                if (packet.getLength() == 0 || packet.getLength() > MAX_UDP_BYTES) continue;
                SeenPeer peer = parseAnnouncement(packet);
                if (peer == null || identity.hostId().equals(peer.hostId)) continue;
                peer.address = formatAddress(packet.getAddress(), peer.port);
                peer.lastAddress = peer.address;
                peer.seenAt = System.currentTimeMillis();
                nearby.put(peer.hostId, peer);
                TrustedPeer paired = trusted.get(peer.hostId);
                if (paired != null && samePairingIdentity(paired, peer)
                        && activeConnections.getOrDefault(peer.hostId, 0) == 0) {
                    if (paired.publicKey.isEmpty()) {
                        trusted.put(peer.hostId, new TrustedPeer(peer.hostId, peer.alias,
                                peer.publicKey, peer.pairingPublicKey, peer.address));
                        persistTrusted();
                    }
                    startOutbound(peer, null);
                }
                publishStatus(ready ? "Running" : startupError);
            } catch (IOException error) {
                if (!stopping && !udp.isClosed()) Log.w(TAG, "Discovery receive failed", error);
            }
        }
    }

    private SeenPeer parseAnnouncement(DatagramPacket packet) {
        try {
            JSONObject json = new JSONObject(new String(packet.getData(), packet.getOffset(),
                    packet.getLength(), StandardCharsets.UTF_8));
            if (!SERVICE_NAME.equals(json.optString("service")) || json.optInt("version", -1) != 1) {
                return null;
            }
            String hostId = json.optString("host_id", "");
            String alias = json.optString("alias", "");
            String networkId = optStringEither(json, "networkId", "network_id");
            String primaryKey = json.optString("public_key", "");
            String pairingKey = optStringEither(json, "pairingPublicKey", "pairing_public_key");
            if (pairingKey.isEmpty() && primaryKey.startsWith("p256:")) pairingKey = primaryKey;
            int port = json.optInt("port", 0);
            String nonce = json.optString("nonce", "");
            long issuedAt = json.optLong("issued_at", -1L);
            long expiresAt = json.optLong("expires_at", -1L);
            long now = System.currentTimeMillis() / 1000L;
            if (hostId.isEmpty() || hostId.length() > 128 || alias.isEmpty() || alias.length() > 128
                    || !NETWORK_ID.equals(networkId)
                    || primaryKey.isEmpty() || primaryKey.length() > 8192
                    || !isP256PublicKey(pairingKey) || port < 1 || port > 65535
                    || nonce.isEmpty() || nonce.length() > 128
                    || issuedAt < 0 || expiresAt < issuedAt || now > expiresAt
                    || issuedAt > now + 60 || expiresAt - issuedAt > 60) {
                return null;
            }
            if (primaryKey.startsWith("p256:")
                    && (!hostId.equals(NodeIdentity.hostIdForPublicKey(primaryKey))
                    || !primaryKey.equals(pairingKey))) {
                return null;
            }
            SeenPeer result = new SeenPeer(hostId, alias, primaryKey, pairingKey, port);
            JSONArray caps = json.optJSONArray("capabilities");
            result.capabilities = caps == null ? Collections.emptyList() : readCapabilities(caps);
            return result;
        } catch (Exception malformed) {
            return null;
        }
    }

    private void announceSafely() {
        try {
            announce();
        } catch (Exception error) {
            Log.w(TAG, "Discovery announcement failed", error);
        }
    }

    private void announce() throws Exception {
        MulticastSocket udp = discoverySocket;
        NodeIdentity currentIdentity = identity;
        NetworkInterface iface = wifiInterface;
        if (!ready || udp == null || currentIdentity == null || iface == null) return;
        long now = System.currentTimeMillis() / 1000L;
        JSONObject json = new JSONObject();
        json.put("service", SERVICE_NAME);
        json.put("version", 1);
        json.put("host_id", currentIdentity.hostId());
        json.put("alias", nodeAlias);
        json.put("os", "android");
        json.put("networkId", NETWORK_ID);
        json.put("port", PEER_PORT);
        json.put("public_key", currentIdentity.publicKey());
        json.put("nonce", randomHex(16));
        json.put("issued_at", now);
        json.put("expires_at", now + ANNOUNCEMENT_TTL_SECONDS);
        JSONArray capabilities = new JSONArray();
        capabilities.put("android-node");
        capabilities.put(STATE_SNAPSHOT_CAPABILITY);
        capabilities.put(P256_PEER_SYNC_CAPABILITY);
        json.put("capabilities", capabilities);
        byte[] bytes = json.toString().getBytes(StandardCharsets.UTF_8);
        InetAddress group = InetAddress.getByName(MULTICAST_ADDRESS);
        udp.send(new DatagramPacket(bytes, bytes.length, group, DISCOVERY_PORT));
        for (InterfaceAddress address : iface.getInterfaceAddresses()) {
            InetAddress broadcast = address.getBroadcast();
            if (address.getAddress() instanceof Inet4Address && broadcast != null) {
                udp.send(new DatagramPacket(bytes, bytes.length, broadcast, DISCOVERY_PORT));
            }
        }
    }

    private void handleInbound(Socket socket) {
        String hostId = null;
        PairingCode.Reservation reservation = null;
        boolean established = false;
        try (Socket connection = socket) {
            connection.setSoTimeout(8000);
            WireMessage helloMessage = readWire(connection.getInputStream());
            if (helloMessage == null || !"AgentHello".equals(helloMessage.type)) {
                sendRejected(connection, "first message must be AgentHello");
                return;
            }
            Hello hello = Hello.from(helloMessage.data);
            hostId = hello.hostId;
            if (!validHello(hello)) {
                sendRejected(connection, "invalid agent identity");
                return;
            }
            TrustedPeer known = trusted.get(hello.hostId);
            boolean isKnown = known != null
                    && (known.publicKey.isEmpty() || known.publicKey.equals(hello.publicKey))
                    && known.pairingPublicKey.equals(hello.authPublicKey());
            if (known != null && !isKnown) {
                sendRejected(connection, "host id is already paired with another key");
                return;
            }
            String challenge = randomHex(32);
            String localPublicKey = identity.publicKey();
            String localPairingKey = localPublicKey;
            byte[] transcript = PairingCode.transcript(hello.hostId, hello.publicKey,
                    hello.authPublicKey(), identity.hostId(), localPublicKey, localPairingKey,
                    NETWORK_ID, challenge);
            sendWire(connection.getOutputStream(), challengeMessage(challenge,
                    identity.hostId(), localPublicKey, localPairingKey,
                    identity.sign(transcript)));
            connection.setSoTimeout(8000);
            WireMessage responseMessage = readWire(connection.getInputStream());
            if (responseMessage == null || !"PairResponse".equals(responseMessage.type)) {
                sendRejected(connection, "expected pair response");
                return;
            }
            String responseHostId = responseMessage.data.optString("host_id", "");
            String responseSignature = responseMessage.data.optString("signature", "");
            String codeProof = responseMessage.data.optString("codeProof", "");
            if (!hello.hostId.equals(responseHostId) || !NodeIdentity.verify(
                    hello.authPublicKey(),
                    transcript, responseSignature)) {
                sendRejected(connection, "challenge signature rejected");
                return;
            }
            if (!isKnown) {
                reservation = pairingCode.reserveProof(codeProof, "initiator", transcript);
                if (reservation == null) {
                    sendRejected(connection, "pairing code proof rejected");
                    return;
                }
            } else if (!codeProof.isEmpty()) {
                sendRejected(connection, "unexpected pairing code proof");
                return;
            }
            String acceptorProof = reservation == null ? null
                    : pairingCode.reservedProof("acceptor", transcript);
            TrustedPeer savedPeer = new TrustedPeer(hello.hostId, hello.alias,
                    hello.publicKey, hello.authPublicKey(), socketAddress(socket.getRemoteSocketAddress()));
            if (!isKnown) {
                synchronized (stateLock) {
                    if (!pairingCode.consume(reservation)) {
                        sendRejected(connection, "pairing code expired or refreshed");
                        return;
                    }
                    trusted.put(hello.hostId, savedPeer);
                    persistTrustedAndCodeLocked();
                }
                reservation = null;
            } else {
                trusted.put(hello.hostId, savedPeer);
                persistTrusted();
            }
            sendWire(connection.getOutputStream(), acceptedMessage(hello.hostId,
                    identity.sign(PairingCode.acceptanceBytes(transcript)), acceptorProof));
            connection.setSoTimeout((int) TimeUnit.SECONDS.toMillis(PEER_TIMEOUT_SECONDS));
            lastError = "";
            markConnected(hello.hostId, true);
            established = true;
            runSession(connection, hello.hostId);
        } catch (Exception error) {
            lastError = safeMessage(error);
            if (!stopping) Log.i(TAG, "Inbound peer session ended: " + safeMessage(error));
        } finally {
            if (reservation != null && pairingCode != null) pairingCode.release(reservation);
            if (established) markConnected(hostId, false);
            connectionSlots.release();
            publishStatus(ready ? "Running" : startupError);
        }
    }

    private boolean validHello(Hello hello) {
        if (hello == null || hello.version != 1 || !NETWORK_ID.equals(hello.networkId)
                || hello.hostId.isEmpty() || hello.hostId.length() > 128
                || hello.alias.isEmpty() || hello.alias.length() > 128
                || hello.publicKey.isEmpty() || hello.publicKey.length() > 8192
                || !isP256PublicKey(hello.authPublicKey())
                || hello.capabilities.size() > 32) {
            return false;
        }
        if (hello.publicKey.startsWith("p256:")) {
            try {
                return hello.hostId.equals(NodeIdentity.hostIdForPublicKey(hello.publicKey))
                        && hello.publicKey.equals(hello.authPublicKey());
            } catch (Exception invalid) {
                return false;
            }
        }
        return true;
    }

    private void startOutbound(SeenPeer peer, String remoteCode) {
        if (!ready || connecting.putIfAbsent(peer.hostId, Boolean.TRUE) != null) return;
        if (activeConnections.getOrDefault(peer.hostId, 0) > 0) {
            connecting.remove(peer.hostId);
            return;
        }
        TrustedPeer existing = trusted.get(peer.hostId);
        if (remoteCode == null && existing == null) {
            connecting.remove(peer.hostId);
            return;
        }
        final String codeForProof = existing == null ? remoteCode : null;
        io.execute(() -> {
            boolean established = false;
            try (Socket socket = new Socket()) {
                InetAddress address = InetAddress.getByName(peer.addressHost());
                SocketAddress target = new InetSocketAddress(address, peer.port);
                socket.connect(target, 5000);
                socket.setSoTimeout(8000);
                sendWire(socket.getOutputStream(), helloMessage());
                WireMessage challengeMessage = readWire(socket.getInputStream());
                if (challengeMessage == null || !"PairChallenge".equals(challengeMessage.type)) {
                    throw new IOException(challengeMessage != null
                            && "PairRejected".equals(challengeMessage.type)
                            ? challengeMessage.data.optString("reason", "pairing rejected")
                            : "peer closed before its challenge");
                }
                String challenge = challengeMessage.data.optString("challenge", "");
                String monitorId = challengeMessage.data.optString("monitor_id", "");
                String signature = challengeMessage.data.optString("signature", "");
                String serverPublicKey = optStringEither(challengeMessage.data,
                        "publicKey", "public_key");
                String challengeKey = optStringEither(challengeMessage.data,
                        "pairingPublicKey", "pairing_public_key");
                if (challengeKey.isEmpty() && peer.publicKey.startsWith("p256:")) {
                    challengeKey = peer.publicKey;
                }
                if (serverPublicKey.isEmpty()) serverPublicKey = peer.publicKey;
                byte[] transcript = PairingCode.transcript(identity.hostId(),
                        identity.publicKey(), identity.publicKey(), monitorId,
                        serverPublicKey, challengeKey, NETWORK_ID, challenge);
                if (!validChallenge(challenge) || !peer.hostId.equals(monitorId)
                        || !serverPublicKey.equals(peer.publicKey)
                        || !isP256PublicKey(challengeKey)
                        || !peer.pairingPublicKey.equals(challengeKey)
                        || !NodeIdentity.verify(challengeKey,
                                transcript, signature)) {
                    throw new IOException("peer challenge signature rejected");
                }
                sendWire(socket.getOutputStream(), pairResponseMessage(identity.hostId(),
                        identity.sign(transcript), codeForProof == null ? null
                                : PairingCode.proof(codeForProof, "initiator", transcript)));
                WireMessage accepted = readWire(socket.getInputStream());
                if (accepted == null || !"PairAccepted".equals(accepted.type)
                        || !identity.hostId().equals(accepted.data.optString("host_id", ""))
                        || !NETWORK_ID.equals(optStringEither(
                                accepted.data, "networkId", "network_id"))) {
                    throw new IOException(accepted != null && "PairRejected".equals(accepted.type)
                            ? accepted.data.optString("reason", "pairing rejected")
                            : "peer did not accept pairing");
                }
                // Acceptance uses a separate signature domain from the challenge.
                if (!NodeIdentity.verify(challengeKey, PairingCode.acceptanceBytes(transcript),
                        accepted.data.optString("signature", ""))) {
                    throw new IOException("PairAccepted signature rejected");
                }
                String acceptedProof = accepted.data.optString("codeProof", "");
                if (codeForProof != null
                        ? !PairingCode.constantTimeEquals(acceptedProof,
                                PairingCode.proof(codeForProof, "acceptor", transcript))
                        : !acceptedProof.isEmpty()) {
                    throw new IOException("peer did not prove possession of pairing code");
                }
                synchronized (stateLock) {
                    trusted.put(peer.hostId, new TrustedPeer(peer.hostId, peer.alias,
                            peer.publicKey, challengeKey, peer.address));
                    persistTrusted();
                }
                lastError = "";
                socket.setSoTimeout((int) TimeUnit.SECONDS.toMillis(PEER_TIMEOUT_SECONDS));
                markConnected(peer.hostId, true);
                established = true;
                runSession(socket, peer.hostId);
            } catch (Exception error) {
                lastError = safeMessage(error);
                if (!stopping) Log.i(TAG, "Peer " + peer.hostId + " connection ended: " + safeMessage(error));
            } finally {
                connecting.remove(peer.hostId);
                if (established) markConnected(peer.hostId, false);
                publishStatus(ready ? "Running" : startupError);
            }
        });
    }

    private void pairManual(String host, int port, String remoteCode) {
        String attemptId = "manual:" + host + ":" + port;
        if (connecting.putIfAbsent(attemptId, Boolean.TRUE) != null) return;
        String peerHostId = null;
        boolean established = false;
        try (Socket socket = new Socket()) {
            socket.connect(new InetSocketAddress(InetAddress.getByName(host), port), 5000);
            socket.setSoTimeout(8000);
            sendWire(socket.getOutputStream(), helloMessage());
            WireMessage challengeMessage = readWire(socket.getInputStream());
            if (challengeMessage == null || !"PairChallenge".equals(challengeMessage.type)) {
                throw new IOException(challengeMessage != null && "PairRejected".equals(challengeMessage.type)
                        ? challengeMessage.data.optString("reason", "pairing rejected")
                        : "peer did not return a pairing challenge");
            }
            String challenge = challengeMessage.data.optString("challenge", "");
            peerHostId = challengeMessage.data.optString("monitor_id", "");
            String signature = challengeMessage.data.optString("signature", "");
            String serverPublicKey = optStringEither(challengeMessage.data, "publicKey", "public_key");
            String challengeKey = optStringEither(challengeMessage.data,
                    "pairingPublicKey", "pairing_public_key");
            if (serverPublicKey.isEmpty()) serverPublicKey = challengeKey;
            byte[] transcript = PairingCode.transcript(identity.hostId(), identity.publicKey(),
                    identity.publicKey(), peerHostId, serverPublicKey, challengeKey,
                    NETWORK_ID, challenge);
            if (!validChallenge(challenge) || peerHostId.isEmpty() || peerHostId.length() > 128
                    || !isP256PublicKey(challengeKey)
                    || !NodeIdentity.verify(challengeKey,
                            transcript, signature)) {
                throw new IOException("peer challenge signature rejected");
            }
            sendWire(socket.getOutputStream(), pairResponseMessage(identity.hostId(),
                    identity.sign(transcript), PairingCode.proof(remoteCode, "initiator", transcript)));
            WireMessage accepted = readWire(socket.getInputStream());
            if (accepted == null || !"PairAccepted".equals(accepted.type)
                    || !identity.hostId().equals(accepted.data.optString("host_id", ""))
                    || !NETWORK_ID.equals(optStringEither(
                            accepted.data, "networkId", "network_id"))) {
                throw new IOException(accepted != null && "PairRejected".equals(accepted.type)
                        ? accepted.data.optString("reason", "pairing rejected")
                        : "peer did not accept pairing");
            }
            if (!NodeIdentity.verify(challengeKey, PairingCode.acceptanceBytes(transcript),
                    accepted.data.optString("signature", ""))
                    || !PairingCode.constantTimeEquals(accepted.data.optString("codeProof", ""),
                            PairingCode.proof(remoteCode, "acceptor", transcript))) {
                throw new IOException("peer did not authenticate pairing acceptance");
            }
            String address = formatAddress(InetAddress.getByName(host), port);
            synchronized (stateLock) {
                trusted.put(peerHostId, new TrustedPeer(peerHostId, peerHostId,
                        "", challengeKey, address));
                persistTrusted();
            }
            lastError = "";
            socket.setSoTimeout((int) TimeUnit.SECONDS.toMillis(PEER_TIMEOUT_SECONDS));
            markConnected(peerHostId, true);
            established = true;
            runSession(socket, peerHostId);
        } catch (Exception error) {
            lastError = safeMessage(error);
            if (!stopping) Log.i(TAG, "Manual peer connection ended: " + lastError);
        } finally {
            connecting.remove(attemptId);
            if (established) markConnected(peerHostId, false);
            publishStatus(ready ? "Running" : startupError);
        }
    }

    private void runSession(Socket socket, String peerHostId) throws Exception {
        activeSockets.add(socket);
        OutputStream output = socket.getOutputStream();
        Object outputLock = new Object();
        ScheduledFuture<?> heartbeats = scheduler.scheduleAtFixedRate(() -> {
            try {
                synchronized (outputLock) {
                    sendWire(output, heartbeatMessage(identity.hostId()));
                }
            } catch (Exception error) {
                closeQuietly(socket);
            }
        }, 10, 15, TimeUnit.SECONDS);
        try {
            while (!stopping && !socket.isClosed()) {
                WireMessage message = readWire(socket.getInputStream());
                if (message == null) return;
                if ("Heartbeat".equals(message.type)) {
                    if (!peerHostId.equals(message.data.optString("host_id", ""))) {
                        throw new IOException("heartbeat host id mismatch");
                    }
                    synchronized (outputLock) {
                        sendWire(output, heartbeatAckMessage());
                    }
                } else if ("AndroidSnapshot".equals(message.type)) {
                    acceptAndroidSnapshot(peerHostId, message.data);
                } else if ("PeerSnapshot".equals(message.type)) {
                    throw new IOException("peer used the wrong snapshot signature capability");
                } else if (!"HeartbeatAck".equals(message.type)
                        && !"StateSnapshot".equals(message.type)) {
                    Log.d(TAG, "Ignoring unsupported peer message " + message.type);
                }
            }
        } finally {
            heartbeats.cancel(false);
            activeSockets.remove(socket);
        }
    }

    private WireMessage helloMessage() throws JSONException {
        JSONObject data = new JSONObject();
        data.put("version", 1);
        data.put("host_id", identity.hostId());
        data.put("alias", nodeAlias);
        data.put("os", "android");
        data.put("networkId", NETWORK_ID);
        data.put("public_key", identity.publicKey());
        // Pairing codes are never sent on the wire; only a transcript HMAC is.
        JSONArray capabilities = new JSONArray();
        capabilities.put("android-node");
        capabilities.put(STATE_SNAPSHOT_CAPABILITY);
        capabilities.put(P256_PEER_SYNC_CAPABILITY);
        data.put("capabilities", capabilities);
        return new WireMessage("AgentHello", data);
    }

    private static WireMessage challengeMessage(String challenge, String monitorId,
            String publicKey, String pairingPublicKey, String signature) throws JSONException {
        JSONObject data = new JSONObject();
        data.put("challenge", challenge);
        data.put("monitor_id", monitorId);
        data.put("signature", signature);
        data.put("publicKey", publicKey);
        data.put("pairingPublicKey", pairingPublicKey);
        return new WireMessage("PairChallenge", data);
    }

    private static WireMessage pairResponseMessage(String hostId, String signature, String codeProof) throws JSONException {
        JSONObject data = new JSONObject();
        data.put("host_id", hostId);
        data.put("signature", signature);
        if (codeProof != null) data.put("codeProof", codeProof);
        return new WireMessage("PairResponse", data);
    }

    private static WireMessage acceptedMessage(String hostId, String signature, String codeProof) throws JSONException {
        JSONObject data = new JSONObject();
        data.put("host_id", hostId);
        data.put("networkId", NETWORK_ID);
        data.put("signature", signature);
        if (codeProof != null) data.put("codeProof", codeProof);
        return new WireMessage("PairAccepted", data);
    }

    private static WireMessage heartbeatMessage(String hostId) throws JSONException {
        JSONObject data = new JSONObject();
        data.put("host_id", hostId);
        return new WireMessage("Heartbeat", data);
    }

    private static WireMessage heartbeatAckMessage() throws JSONException {
        return new WireMessage("HeartbeatAck", new JSONObject());
    }

    private void acceptAndroidSnapshot(String peerHostId, JSONObject data) throws Exception {
        TrustedPeer peer = trusted.get(peerHostId);
        String networkId = data.optString("networkId", "");
        String senderId = data.optString("senderId", "");
        String payloadJson = data.optString("payloadJson", "");
        String signature = data.optString("signature", "");
        if (peer == null || !NETWORK_ID.equals(networkId) || !peerHostId.equals(senderId)
                || payloadJson.isEmpty() || payloadJson.length() > MAX_WIRE_BYTES) {
            throw new IOException("Android snapshot envelope rejected");
        }
        String prefix = "codex-monitor/android-snapshot-v1:" + networkId + ":" + senderId + ":";
        byte[] prefixBytes = prefix.getBytes(StandardCharsets.UTF_8);
        byte[] payloadBytes = payloadJson.getBytes(StandardCharsets.UTF_8);
        byte[] signed = new byte[prefixBytes.length + payloadBytes.length];
        System.arraycopy(prefixBytes, 0, signed, 0, prefixBytes.length);
        System.arraycopy(payloadBytes, 0, signed, prefixBytes.length, payloadBytes.length);
        if (!NodeIdentity.verify(peer.pairingPublicKey, signed, signature)) {
            throw new IOException("Android snapshot signature rejected");
        }
        JSONObject payload = new JSONObject(payloadJson);
        if (!peerHostId.equals(payload.optString("hostId"))
                || !peerHostId.equals(payload.optString("sourceNodeId", peerHostId))
                || !NETWORK_ID.equals(payload.optString("networkId", NETWORK_ID))) {
            throw new IOException("Android snapshot source rejected");
        }
        MonitorState state = new MonitorState(payload);
        if (state.sequence <= 0 || state.capturedAt <= 0 || state.alias.isEmpty()
                || state.alias.length() > 128 || state.sessions.length() > 25) {
            throw new IOException("Android snapshot limits rejected");
        }
        MonitorState previous = monitorStates.get(peerHostId);
        if (previous == null || state.sequence > previous.sequence
                || (state.sequence == previous.sequence && state.capturedAt > previous.capturedAt)) {
            monitorStates.put(peerHostId, state);
            persistMonitorStates();
            publishStatus(ready ? "Running" : startupError);
        }
    }

    private void sendRejected(Socket socket, String reason) {
        try {
            JSONObject data = new JSONObject();
            data.put("reason", reason);
            sendWire(socket.getOutputStream(), new WireMessage("PairRejected", data));
        } catch (Exception ignored) { }
    }

    private static WireMessage readWire(InputStream input) throws IOException, JSONException {
        ByteArrayOutputStream bytes = new ByteArrayOutputStream();
        while (bytes.size() <= MAX_WIRE_BYTES) {
            int value = input.read();
            if (value < 0) return bytes.size() == 0 ? null : parseWire(bytes.toByteArray());
            if (value == '\n') return parseWire(bytes.toByteArray());
            if (value != '\r') bytes.write(value);
        }
        throw new IOException("wire frame exceeds maximum size");
    }

    private static WireMessage parseWire(byte[] bytes) throws JSONException, IOException {
        if (bytes.length == 0 || bytes.length > MAX_WIRE_BYTES) {
            throw new IOException("empty or oversized wire frame");
        }
        JSONObject json = new JSONObject(new String(bytes, StandardCharsets.UTF_8));
        String type = json.optString("type", "");
        JSONObject data = json.optJSONObject("data");
        if ("HeartbeatAck".equals(type) && data == null) data = new JSONObject();
        if (type.isEmpty() || data == null) throw new IOException("invalid wire message");
        return new WireMessage(type, data);
    }

    private static void sendWire(OutputStream output, WireMessage message)
            throws IOException, JSONException {
        byte[] bytes = message.json().toString().getBytes(StandardCharsets.UTF_8);
        if (bytes.length == 0 || bytes.length > MAX_WIRE_BYTES) {
            throw new IOException("outgoing wire frame is oversized");
        }
        output.write(bytes);
        output.write('\n');
        output.flush();
    }

    private void loadMonitorStates() {
        String encoded = getSharedPreferences(PREFS, MODE_PRIVATE).getString(PREF_SNAPSHOTS, "[]");
        try {
            JSONArray values = new JSONArray(encoded);
            for (int i = 0; i < values.length(); i++) {
                MonitorState state = new MonitorState(values.getJSONObject(i));
                monitorStates.put(state.hostId, state);
            }
        } catch (Exception error) {
            Log.w(TAG, "Saved monitor snapshots are malformed; ignoring them", error);
            monitorStates.clear();
        }
    }

    private void persistMonitorStates() {
        JSONArray values = new JSONArray();
        for (MonitorState state : monitorStates.values()) {
            try { values.put(state.json()); } catch (JSONException ignored) { }
        }
        getSharedPreferences(PREFS, MODE_PRIVATE).edit()
                .putString(PREF_SNAPSHOTS, values.toString()).apply();
    }

    private void loadTrustedPeers() {
        String encoded = getSharedPreferences(PREFS, MODE_PRIVATE).getString(PREF_TRUSTED, "[]");
        try {
            JSONArray peers = new JSONArray(encoded);
            for (int i = 0; i < peers.length() && i < 256; i++) {
                JSONObject value = peers.optJSONObject(i);
                if (value == null) continue;
                String hostId = value.optString("hostId", "");
                String publicKey = value.optString("publicKey", "");
                String pairingKey = value.optString("pairingPublicKey", "");
                String alias = value.optString("alias", "");
                if (!hostId.isEmpty() && hostId.length() <= 128
                        && publicKey.length() <= 8192 && isP256PublicKey(pairingKey)) {
                    trusted.put(hostId, new TrustedPeer(hostId,
                            alias.isEmpty() ? hostId : alias, publicKey, pairingKey,
                            value.optString("lastAddress", "")));
                }
            }
        } catch (JSONException error) {
            Log.w(TAG, "Saved trust list is malformed; ignoring it", error);
        }
    }

    private void persistTrusted() {
        synchronized (stateLock) {
            JSONArray peers = new JSONArray();
            for (TrustedPeer peer : trusted.values()) {
                try {
                    JSONObject value = new JSONObject();
                    value.put("hostId", peer.hostId);
                    value.put("alias", peer.alias);
                    value.put("publicKey", peer.publicKey);
                    value.put("pairingPublicKey", peer.pairingPublicKey);
                    value.put("lastAddress", peer.lastAddress);
                    peers.put(value);
                } catch (JSONException ignored) { }
            }
            getSharedPreferences(PREFS, MODE_PRIVATE).edit()
                    .putString(PREF_TRUSTED, peers.toString()).commit();
        }
    }

    private void persistTrustedAndCodeLocked() {
        JSONArray peers = new JSONArray();
        for (TrustedPeer peer : trusted.values()) {
            try {
                JSONObject value = new JSONObject();
                value.put("hostId", peer.hostId);
                value.put("alias", peer.alias);
                value.put("publicKey", peer.publicKey);
                value.put("pairingPublicKey", peer.pairingPublicKey);
                value.put("lastAddress", peer.lastAddress);
                peers.put(value);
            } catch (JSONException ignored) { }
        }
        getSharedPreferences(PREFS, MODE_PRIVATE).edit()
                .putString(PREF_TRUSTED, peers.toString())
                .putString(PREF_CODE, pairingCode.current())
                .commit();
    }

    private void persistPairingCode() {
        PairingCode current = pairingCode;
        if (current != null) {
            getSharedPreferences(PREFS, MODE_PRIVATE).edit()
                    .putString(PREF_CODE, current.current()).commit();
        }
    }

    private void expireOldDiscoveries() {
        long cutoff = System.currentTimeMillis() - TimeUnit.SECONDS.toMillis(ANNOUNCEMENT_TTL_SECONDS * 2);
        boolean changed = false;
        for (Map.Entry<String, SeenPeer> entry : nearby.entrySet()) {
            if (entry.getValue().seenAt < cutoff) {
                changed |= nearby.remove(entry.getKey(), entry.getValue());
            }
        }
        if (changed) publishStatus(ready ? "Running" : startupError);
    }

    private void markConnected(String hostId, boolean connected) {
        if (hostId == null) return;
        synchronized (activeConnections) {
            int count = activeConnections.getOrDefault(hostId, 0);
            if (connected) activeConnections.put(hostId, count + 1);
            else if (count <= 1) activeConnections.remove(hostId);
            else activeConnections.put(hostId, count - 1);
        }
        publishStatus(ready ? "Running" : startupError);
    }

    private void publishStatus(String status) {
        startupError = status;
        mainHandler.post(() -> {
            ChangeListener listener = changeListener;
            if (listener != null) listener.onNodeStateChanged();
            if (ready) {
                ((NotificationManager) getSystemService(NOTIFICATION_SERVICE))
                        .notify(1, makeNotification("Node online · TCP 8787 · UDP 37991"));
            } else {
                ((NotificationManager) getSystemService(NOTIFICATION_SERVICE))
                        .notify(1, makeNotification(status));
            }
        });
    }

    private void createNotificationChannel() {
        if (Build.VERSION.SDK_INT >= 26) {
            NotificationChannel channel = new NotificationChannel("node-service",
                    "Codex Monitor node", NotificationManager.IMPORTANCE_LOW);
            channel.setDescription("Persistent local peer discovery and pairing status");
            ((NotificationManager) getSystemService(NOTIFICATION_SERVICE)).createNotificationChannel(channel);
        }
    }

    private Notification makeNotification(String text) {
        Intent openIntent = new Intent(this, MainActivity.class);
        PendingIntent pending = PendingIntent.getActivity(this, 0, openIntent,
                PendingIntent.FLAG_UPDATE_CURRENT | (Build.VERSION.SDK_INT >= 23
                        ? PendingIntent.FLAG_IMMUTABLE : 0));
        Notification.Builder builder = Build.VERSION.SDK_INT >= 26
                ? new Notification.Builder(this, "node-service") : new Notification.Builder(this);
        return builder.setSmallIcon(android.R.drawable.stat_sys_upload_done)
                .setContentTitle("Codex Monitor node")
                .setContentText(text)
                .setOngoing(true)
                .setContentIntent(pending)
                .build();
    }

    private static NetworkInterface findWifiInterface() throws SocketException {
        Enumeration<NetworkInterface> interfaces = NetworkInterface.getNetworkInterfaces();
        if (interfaces == null) return null;
        while (interfaces.hasMoreElements()) {
            NetworkInterface iface = interfaces.nextElement();
            String name = iface.getName().toLowerCase(java.util.Locale.ROOT);
            if ((name.startsWith("wlan") || name.startsWith("wifi"))
                    && usableWifiInterface(iface)) return iface;
        }
        return null;
    }

    private static boolean usableWifiInterface(NetworkInterface iface) throws SocketException {
        if (!iface.isUp() || iface.isLoopback() || !iface.supportsMulticast()) return false;
        Enumeration<InetAddress> addresses = iface.getInetAddresses();
        while (addresses.hasMoreElements()) {
            if (addresses.nextElement() instanceof Inet4Address) return true;
        }
        return false;
    }

    private static boolean isP256PublicKey(String key) {
        if (key == null || !key.startsWith("p256:") || key.length() > 4101) return false;
        try {
            NodeIdentity.hostIdForPublicKey(key);
            return true;
        } catch (Exception invalid) {
            return false;
        }
    }

    private static String[] parseEndpoint(String value) {
        if (value == null || value.trim().isEmpty() || value.length() > 300
                || value.indexOf('/') >= 0 || value.indexOf('@') >= 0
                || value.matches(".*\\s+.*")) {
            throw new IllegalArgumentException("请输入主机名或 IP 地址，可选填写 :8787 端口。");
        }
        String host;
        String portText = Integer.toString(PEER_PORT);
        String input = value.trim();
        if (input.startsWith("[")) {
            int close = input.indexOf(']');
            if (close < 2) throw new IllegalArgumentException("方括号中的 IPv6 地址无效。");
            host = input.substring(1, close);
            String suffix = input.substring(close + 1);
            if (!suffix.isEmpty()) {
                if (!suffix.startsWith(":")) throw new IllegalArgumentException("地址端口格式无效。");
                portText = suffix.substring(1);
            }
        } else {
            int firstColon = input.indexOf(':');
            int lastColon = input.lastIndexOf(':');
            if (firstColon >= 0 && firstColon == lastColon) {
                host = input.substring(0, firstColon);
                portText = input.substring(firstColon + 1);
            } else {
                host = input;
            }
        }
        int port;
        try { port = Integer.parseInt(portText); }
        catch (NumberFormatException invalid) {
            throw new IllegalArgumentException("端口必须是 1 到 65535 之间的数字。");
        }
        if (host.isEmpty() || host.length() > 253 || port < 1 || port > 65535) {
            throw new IllegalArgumentException("请输入有效主机及 1 到 65535 之间的端口。");
        }
        return new String[]{host, Integer.toString(port)};
    }

    private static boolean samePairingIdentity(TrustedPeer known, SeenPeer announced) {
        return (known.publicKey.isEmpty() || known.publicKey.equals(announced.publicKey))
                && known.pairingPublicKey.equals(announced.pairingPublicKey);
    }

    private static List<String> readCapabilities(JSONArray values) {
        ArrayList<String> result = new ArrayList<>();
        for (int i = 0; i < values.length() && i < 32; i++) {
            String value = values.optString(i, "");
            if (!value.isEmpty() && value.length() <= 64) result.add(value);
        }
        return result;
    }

    private static String optStringEither(JSONObject json, String primary, String fallback) {
        String value = json.optString(primary, "");
        return value.isEmpty() ? json.optString(fallback, "") : value;
    }

    private String randomHex(int count) {
        byte[] bytes = new byte[count];
        random.nextBytes(bytes);
        return NodeIdentity.hex(bytes);
    }

    private static String formatAddress(InetAddress address, int port) {
        String host = address.getHostAddress();
        return (address instanceof Inet4Address ? host : "[" + host + "]") + ":" + port;
    }

    private static String socketAddress(SocketAddress address) {
        if (address instanceof InetSocketAddress) {
            InetSocketAddress socket = (InetSocketAddress) address;
            return formatAddress(socket.getAddress(), socket.getPort());
        }
        return "";
    }

    private static String safeMessage(Throwable error) {
        String message = error.getMessage();
        if (message == null || message.isEmpty()) return error.getClass().getSimpleName();
        return message.length() > 180 ? message.substring(0, 180) : message;
    }

    private static boolean validChallenge(String value) {
        if (value == null || value.length() != 64) return false;
        for (int i = 0; i < value.length(); i++) {
            char c = value.charAt(i);
            if (!(c >= '0' && c <= '9') && !(c >= 'a' && c <= 'f')) return false;
        }
        return true;
    }

    private static void closeQuietly(Socket socket) {
        try { socket.close(); } catch (IOException ignored) { }
    }

    private static void closeQuietly(ServerSocket socket) {
        try { if (socket != null) socket.close(); } catch (IOException ignored) { }
    }

    private static void closeQuietly(MulticastSocket socket) {
        if (socket != null) socket.close();
    }

    private static ThreadFactory namedThreads(String prefix) {
        return runnable -> {
            Thread thread = new Thread(runnable, prefix);
            thread.setDaemon(true);
            return thread;
        };
    }

    private static final class WireMessage {
        final String type;
        final JSONObject data;

        WireMessage(String type, JSONObject data) {
            this.type = type;
            this.data = data;
        }

        JSONObject json() throws JSONException {
            JSONObject root = new JSONObject();
            root.put("type", type);
            if (!"HeartbeatAck".equals(type)) root.put("data", data);
            return root;
        }
    }

    private static final class Hello {
        int version;
        String hostId;
        String alias;
        String publicKey;
        String pairingPublicKey;
        String networkId;
        List<String> capabilities;

        static Hello from(JSONObject data) {
            Hello hello = new Hello();
            hello.version = data.optInt("version", -1);
            hello.hostId = data.optString("host_id", "");
            hello.alias = data.optString("alias", "");
            hello.publicKey = data.optString("public_key", "");
            hello.pairingPublicKey = optStringEither(data, "pairingPublicKey", "pairing_public_key");
            hello.networkId = optStringEither(data, "networkId", "network_id");
            JSONArray values = data.optJSONArray("capabilities");
            hello.capabilities = values == null ? Collections.emptyList() : readCapabilities(values);
            return hello;
        }

        String authPublicKey() {
            return pairingPublicKey == null || pairingPublicKey.isEmpty()
                    ? publicKey : pairingPublicKey;
        }
    }

    private static class SeenPeer {
        final String hostId;
        final String alias;
        final String publicKey;
        final String pairingPublicKey;
        final int port;
        volatile String address = "";
        volatile String lastAddress = "";
        volatile long seenAt;
        List<String> capabilities = Collections.emptyList();

        SeenPeer(String hostId, String alias, String publicKey, String pairingPublicKey, int port) {
            this.hostId = hostId;
            this.alias = alias;
            this.publicKey = publicKey;
            this.pairingPublicKey = pairingPublicKey;
            this.port = port;
        }

        String addressHost() {
            String value = address;
            if (value.startsWith("[")) return value.substring(1, value.indexOf(']'));
            int separator = value.lastIndexOf(':');
            return separator < 0 ? value : value.substring(0, separator);
        }
    }

    private static final class TrustedPeer {
        final String hostId;
        final String alias;
        final String publicKey;
        final String pairingPublicKey;
        final String lastAddress;

        TrustedPeer(String hostId, String alias, String publicKey, String pairingPublicKey, String lastAddress) {
            this.hostId = hostId;
            this.alias = alias;
            this.publicKey = publicKey;
            this.pairingPublicKey = pairingPublicKey;
            this.lastAddress = lastAddress == null ? "" : lastAddress;
        }

        TrustedPeer withAddress(String address) {
            return new TrustedPeer(hostId, alias, publicKey, pairingPublicKey, address);
        }
    }
}
