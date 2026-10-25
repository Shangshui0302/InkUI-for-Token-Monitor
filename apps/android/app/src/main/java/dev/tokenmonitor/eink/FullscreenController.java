package dev.tokenmonitor.eink;

import android.app.Activity;
import android.content.Context;
import android.content.SharedPreferences;
import android.os.Build;
import android.view.View;
import android.view.Window;
import android.view.WindowInsets;
import android.view.WindowInsetsController;
import android.view.WindowManager;
import java.util.function.Consumer;

/** Persistent immersive preference, with separate API 29 and API 30+ inset handling. */
final class FullscreenController {
    private final Window window;
    private final View content;
    private final SharedPreferences preferences;
    private final Consumer<Boolean> listener;
    private boolean enabled;
    private boolean active;

    FullscreenController(Activity activity, View content, Consumer<Boolean> listener) {
        this.window = activity.getWindow();
        this.content = content;
        this.listener = listener;
        preferences = activity.getSharedPreferences("device_ui", Context.MODE_PRIVATE);
        enabled = preferences.getBoolean("fullscreen", true);
        // FLAG_FULLSCREEN disables adjustResize. Immersive system-bar APIs do not need that flag.
        window.clearFlags(WindowManager.LayoutParams.FLAG_FULLSCREEN | WindowManager.LayoutParams.FLAG_LAYOUT_NO_LIMITS);
        window.setSoftInputMode(WindowManager.LayoutParams.SOFT_INPUT_ADJUST_RESIZE);
        if (Build.VERSION.SDK_INT >= 30) {
            window.setDecorFitsSystemWindows(false);
            content.setOnApplyWindowInsetsListener((view, insets) -> {
                int handledTypes = WindowInsets.Type.systemBars() | WindowInsets.Type.ime()
                        | WindowInsets.Type.displayCutout();
                android.graphics.Insets safe = insets.getInsets(handledTypes);
                view.setPadding(safe.left, safe.top, safe.right, safe.bottom);
                // Resize the WebView's native container, then zero handled insets for Chromium.
                // Pass updates through (including IME close), avoiding duplicate or ghost padding.
                return new WindowInsets.Builder(insets).setInsets(handledTypes, android.graphics.Insets.NONE).build();
            });
        } else {
            // Android 10 fits visible system bars and resizes for IME itself. Do not add IME padding twice
            // or use LAYOUT_FULLSCREEN/LAYOUT_HIDE_NAVIGATION, which would put the form behind bars.
            WindowManager.LayoutParams attributes = window.getAttributes();
            attributes.layoutInDisplayCutoutMode = WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_NEVER;
            window.setAttributes(attributes);
        }
    }

    boolean isEnabled() { return enabled; }

    void setEnabled(boolean value) {
        if (enabled == value) return;
        enabled = value;
        preferences.edit().putBoolean("fullscreen", value).apply();
        apply();
        listener.accept(value);
    }

    void resume() { active = true; apply(); }
    void pause() { active = false; }
    void focusChanged(boolean focused) { if (focused && active) apply(); }
    void configurationChanged() { if (active) apply(); }
    void destroy() { active = false; content.setOnApplyWindowInsetsListener(null); }

    @SuppressWarnings("deprecation")
    private void apply() {
        if (!active) return;
        if (Build.VERSION.SDK_INT >= 30) {
            WindowInsetsController controller = window.getInsetsController();
            if (controller != null) {
                controller.setSystemBarsBehavior(WindowInsetsController.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE);
                if (enabled) controller.hide(WindowInsets.Type.systemBars());
                else controller.show(WindowInsets.Type.systemBars());
            }
        } else {
            // Without layout-under-bars flags, the platform owns the safe frame in API 29.
            View decor = window.getDecorView();
            int owned = View.SYSTEM_UI_FLAG_IMMERSIVE | View.SYSTEM_UI_FLAG_IMMERSIVE_STICKY
                    | View.SYSTEM_UI_FLAG_FULLSCREEN | View.SYSTEM_UI_FLAG_HIDE_NAVIGATION
                    | View.SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN | View.SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION
                    | View.SYSTEM_UI_FLAG_LAYOUT_STABLE;
            int visibility = decor.getSystemUiVisibility() & ~owned;
            // Preserve theme appearance flags, so exiting fullscreen retains dark icons on white bars.
            if (enabled) visibility |= View.SYSTEM_UI_FLAG_IMMERSIVE_STICKY
                    | View.SYSTEM_UI_FLAG_FULLSCREEN | View.SYSTEM_UI_FLAG_HIDE_NAVIGATION;
            decor.setSystemUiVisibility(visibility);
        }
        content.requestApplyInsets();
    }
}
