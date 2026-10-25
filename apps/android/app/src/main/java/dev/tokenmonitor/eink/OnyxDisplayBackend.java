package dev.tokenmonitor.eink;

import java.lang.reflect.Method;

/** Optional platform adapter: no SDK dependency, hidden-API bypass, or system-wide settings. */
final class OnyxDisplayBackend {
    /*
     * Verified read-only on Poke3, 2026-10-25:
     * ONYX/Poke3/Poke3:10/2023-12-12_23-45_3.5_4687b1901/12699:user/dev-keys
     * /system/framework/framework.jar SHA256:
     * 31d50cf17a3bc090a4d8c21bc6fc63d2e3994554c07a098658c442334b346521
     * android.onyx.ViewUpdateHelper: GC16=2, PARTIAL=0, FULL=32.
     * android.view.View: public getDefaultUpdateMode()/setDefaultUpdateMode(int).
     * View.invalidate() synchronously reads that default and forwards it to invalidate(boolean,int).
     * Unlike these two accessors, invalidate(int) and ViewUpdateHelper are hidden-API BLOCKED.
     * Thus scope the mode to one ordinary invalidate(), restoring the previous mode immediately.
     * Onyx's official mode semantics (GU=16-gray partial, GC=16-gray full):
     * https://github.com/onyx-intl/OnyxAndroidDemo/blob/ec512c79c0d39c1e294214d36580ce4c914bbea6/doc/EPD-Update-Mode.md
     */
    static final int GRAY_PARTIAL = 2;
    static final int GRAY_FULL = 2 | 32;
    static final String METHOD = "onyx-view-default-update-mode";

    private final Object view;
    private Method getMode;
    private Method setMode;
    private Method invalidate;
    private volatile boolean supported;

    OnyxDisplayBackend(String manufacturer, String model, Object view, Class<?> viewType) {
        this.view = view;
        // Other vendors/models can use different mode bitfields. Fail closed outside the proven target.
        if (!"ONYX".equalsIgnoreCase(manufacturer) || !"Poke3".equalsIgnoreCase(model)) return;
        try {
            getMode = viewType.getMethod("getDefaultUpdateMode");
            setMode = viewType.getMethod("setDefaultUpdateMode", int.class);
            invalidate = viewType.getMethod("invalidate");
            supported = getMode.getReturnType() == int.class && setMode.getReturnType() == void.class
                    && invalidate.getReturnType() == void.class && viewType.isInstance(view);
        } catch (ReflectiveOperationException | RuntimeException | LinkageError ignored) { supported = false; }
    }

    boolean isSupported() { return supported; }
    String method() { return supported ? METHOD : "system-redraw"; }

    /** True means the platform request was dispatched; it is not proof of the panel waveform. */
    boolean refresh(boolean full) {
        if (!supported) return false;
        Integer original = null;
        boolean success = false;
        try {
            original = (Integer) getMode.invoke(view);
            setMode.invoke(view, full ? GRAY_FULL : GRAY_PARTIAL);
            invalidate.invoke(view);
            success = true;
        } catch (ReflectiveOperationException | RuntimeException | LinkageError ignored) { supported = false; }
        finally {
            if (original != null) {
                try { setMode.invoke(view, original); }
                catch (ReflectiveOperationException | RuntimeException | LinkageError ignored) { supported = false; success = false; }
            }
        }
        return success;
    }
}
