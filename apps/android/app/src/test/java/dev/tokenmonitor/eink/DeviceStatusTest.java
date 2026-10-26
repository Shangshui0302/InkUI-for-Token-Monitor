package dev.tokenmonitor.eink;

import java.time.Instant;
import java.time.ZoneId;
import java.util.Arrays;
import java.util.Collections;

/** Value-level checks without Android framework mocks or a device. */
public final class DeviceStatusTest {
    public static void main(String[] args) {
        long now = Instant.parse("2026-10-25T15:59:42Z").toEpochMilli();
        DeviceStatus.Link lan = new DeviceStatus.Link("wifi", false);
        DeviceStatus status = new DeviceStatus(true, now, ZoneId.of("Asia/Shanghai"), 73, false, lan);
        equal("{\"fullscreen\":true,\"time\":\"23:59\",\"batteryPercent\":73,\"charging\":false,\"network\":\"wifi\",\"networkValidated\":false,\"networkRevision\":\"0\"}", status.toJson());
        equal("00:00", new DeviceStatus(true, now + 18_000, ZoneId.of("Asia/Shanghai"), 73, false, lan).time);
        equal("15:59", new DeviceStatus(true, now, ZoneId.of("UTC"), 73, false, lan).time);
        equal(18_000L, DeviceStatus.millisToNextMinute(now));
        equal(60_000L, DeviceStatus.millisToNextMinute(now + 18_000));
        equal(1L, DeviceStatus.millisToNextMinute(now + 17_999));

        equal(50, DeviceStatus.batteryPercent(100, 200));
        equal(0, DeviceStatus.batteryPercent(0, 100));
        equal(100, DeviceStatus.batteryPercent(100, 100));
        equal(null, DeviceStatus.batteryPercent(-1, 100));
        equal(null, DeviceStatus.batteryPercent(50, -1));
        equal(null, DeviceStatus.batteryPercent(50, 0));
        equal(null, DeviceStatus.batteryPercent(101, 100));

        // Public internet validation must never turn a connected LAN into offline.
        equal(lan, DeviceStatus.Link.select(Collections.singletonList(lan), true));
        equal(DeviceStatus.Link.OFFLINE, DeviceStatus.Link.select(Collections.emptyList(), true));
        equal(DeviceStatus.Link.UNKNOWN, DeviceStatus.Link.select(Collections.emptyList(), false));
        equal(DeviceStatus.Link.UNKNOWN, DeviceStatus.Link.select(Collections.singletonList(null), true));
        equal(lan, DeviceStatus.Link.select(Arrays.asList(new DeviceStatus.Link("cellular", true), lan), true));
        equal(lan, DeviceStatus.Link.select(Arrays.asList(lan, new DeviceStatus.Link("cellular", true)), true));
        DeviceStatus.Link wired = new DeviceStatus.Link("ethernet", false);
        equal(wired, DeviceStatus.Link.select(Arrays.asList(lan, wired), true));
        equal(new DeviceStatus.Link("wifi", true), DeviceStatus.Link.select(Arrays.asList(lan, new DeviceStatus.Link("wifi", true)), true));
        equal(DeviceStatus.Link.UNKNOWN, DeviceStatus.Link.select(Collections.singletonList(lan), false));

        // Seconds do not change the emitted value; each meaningful field does.
        equal(status, new DeviceStatus(true, now + 1_000, ZoneId.of("Asia/Shanghai"), 73, false, lan));
        different(status, new DeviceStatus(false, now, ZoneId.of("Asia/Shanghai"), 73, false, lan));
        different(status, new DeviceStatus(true, now + 60_000, ZoneId.of("Asia/Shanghai"), 73, false, lan));
        different(status, new DeviceStatus(true, now, ZoneId.of("Asia/Shanghai"), null, false, lan));
        different(status, new DeviceStatus(true, now, ZoneId.of("Asia/Shanghai"), 73, true, lan));
        different(status, new DeviceStatus(true, now, ZoneId.of("Asia/Shanghai"), 73, false, new DeviceStatus.Link("wifi", true)));
        different(status, new DeviceStatus(true, now, ZoneId.of("Asia/Shanghai"), 73, false, lan, 1));
        DeviceStatus changedNetwork = new DeviceStatus(true, now, ZoneId.of("Asia/Shanghai"), 73, false, lan, 42);
        if (!changedNetwork.toJson().contains("\"networkRevision\":\"42\"")) throw new AssertionError("Missing network revision");
        DeviceStatus unknown = new DeviceStatus(false, now, ZoneId.of("UTC"), null, null, DeviceStatus.Link.UNKNOWN);
        equal("{\"fullscreen\":false,\"time\":\"15:59\",\"batteryPercent\":null,\"charging\":null,\"network\":\"unknown\",\"networkValidated\":null,\"networkRevision\":\"0\"}", unknown.toJson());
        System.out.println("Device status checks passed (clock, battery, LAN link, unknown values, change equality)");
    }

    private static void equal(Object expected, Object actual) {
        if (!java.util.Objects.equals(expected, actual)) throw new AssertionError(expected + " != " + actual);
    }
    private static void different(Object first, Object second) {
        if (first.equals(second)) throw new AssertionError("Meaningful change was lost");
    }
}
