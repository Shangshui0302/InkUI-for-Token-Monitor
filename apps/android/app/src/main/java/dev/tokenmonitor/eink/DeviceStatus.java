package dev.tokenmonitor.eink;

import java.time.Instant;
import java.time.ZoneId;
import java.time.format.DateTimeFormatter;
import java.util.Locale;
import java.util.Objects;

/** Immutable, non-sensitive bridge value. It never contains network identifiers. */
final class DeviceStatus {
    private static final DateTimeFormatter TIME = DateTimeFormatter.ofPattern("HH:mm", Locale.ROOT);
    final boolean fullscreen;
    final String time;
    final Integer batteryPercent;
    final Boolean charging;
    final Link link;
    final long networkRevision;

    DeviceStatus(boolean fullscreen, long now, ZoneId zone, Integer batteryPercent, Boolean charging, Link link) {
        this(fullscreen, now, zone, batteryPercent, charging, link, 0);
    }

    DeviceStatus(boolean fullscreen, long now, ZoneId zone, Integer batteryPercent, Boolean charging, Link link, long networkRevision) {
        this.fullscreen = fullscreen;
        this.time = TIME.format(Instant.ofEpochMilli(now).atZone(zone));
        this.batteryPercent = batteryPercent;
        this.charging = charging;
        this.link = link;
        this.networkRevision = networkRevision;
    }

    String toJson() {
        return "{\"fullscreen\":" + fullscreen + ",\"time\":\"" + time
                + "\",\"batteryPercent\":" + batteryPercent + ",\"charging\":" + charging
                + ",\"network\":\"" + link.transport + "\",\"networkValidated\":" + link.validated
                + ",\"networkRevision\":\"" + networkRevision + "\"}";
    }

    static Integer batteryPercent(int level, int scale) {
        if (scale <= 0 || level < 0 || level > scale) return null;
        return (int) Math.round(level * 100.0 / scale);
    }

    static long millisToNextMinute(long now) { return 60_000L - Math.floorMod(now, 60_000L); }

    @Override public boolean equals(Object other) {
        if (!(other instanceof DeviceStatus)) return false;
        DeviceStatus value = (DeviceStatus) other;
        return fullscreen == value.fullscreen && time.equals(value.time)
                && Objects.equals(batteryPercent, value.batteryPercent) && Objects.equals(charging, value.charging)
                && link.equals(value.link) && networkRevision == value.networkRevision;
    }
    @Override public int hashCode() { return Objects.hash(fullscreen, time, batteryPercent, charging, link, networkRevision); }

    static final class Link {
        static final Link UNKNOWN = new Link("unknown", null);
        static final Link OFFLINE = new Link("offline", false);
        final String transport;
        final Boolean validated;
        Link(String transport, Boolean validated) { this.transport = transport; this.validated = validated; }

        // Prefer wired/Wi-Fi links, including LAN-only links, over cellular. Validation is independent.
        static Link select(Iterable<Link> links, boolean readable) {
            if (!readable) return UNKNOWN;
            Link selected = OFFLINE;
            for (Link link : links) {
                if (link == null) link = UNKNOWN;
                if (rank(link) > rank(selected)
                        || (rank(link) == rank(selected) && Boolean.TRUE.equals(link.validated))) selected = link;
            }
            return selected;
        }
        private static int rank(Link link) {
            switch (link.transport) {
                case "ethernet": return 5;
                case "wifi": return 4;
                case "cellular": return 3;
                case "other": return 2;
                case "unknown": return 1;
                default: return 0;
            }
        }
        @Override public boolean equals(Object other) {
            return other instanceof Link && transport.equals(((Link) other).transport)
                    && Objects.equals(validated, ((Link) other).validated);
        }
        @Override public int hashCode() { return Objects.hash(transport, validated); }
    }
}
