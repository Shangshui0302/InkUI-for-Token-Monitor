package dev.nekodex.platformlifecycle

import android.Manifest
import android.app.Activity
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.wifi.WifiManager
import android.os.Build
import androidx.core.app.ActivityCompat
import androidx.core.content.ContextCompat
import app.tauri.annotation.Command
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.Plugin

@TauriPlugin
class PlatformLifecyclePlugin(private val activity: Activity) : Plugin(activity) {
    private var discoveryMulticastLock: WifiManager.MulticastLock? = null

    @Command
    fun startCoreService(invoke: Invoke) {
        try {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU &&
                ContextCompat.checkSelfPermission(activity, Manifest.permission.POST_NOTIFICATIONS) !=
                PackageManager.PERMISSION_GRANTED
            ) {
                ActivityCompat.requestPermissions(
                    activity,
                    arrayOf(Manifest.permission.POST_NOTIFICATIONS),
                    NOTIFICATION_PERMISSION_REQUEST,
                )
            }
            CoreForegroundService.confirmCoreAvailable()
            val intent = Intent(activity.applicationContext, CoreForegroundService::class.java)
            ContextCompat.startForegroundService(activity.applicationContext, intent)
            invoke.resolve()
        } catch (_: Exception) {
            CoreForegroundService.confirmCoreUnavailable()
            invoke.reject("Android core foreground service failed to start")
        }
    }

    @Command
    fun stopCoreService(invoke: Invoke) {
        try {
            CoreForegroundService.confirmCoreUnavailable()
            val intent = Intent(activity.applicationContext, CoreForegroundService::class.java)
            activity.applicationContext.stopService(intent)
            invoke.resolve()
        } catch (_: Exception) {
            invoke.reject("Android core foreground service failed to stop")
        }
    }

    @Command
    fun acquireDiscoveryMulticastLock(invoke: Invoke) {
        try {
            releaseDiscoveryLock()
            val wifi = activity.applicationContext.getSystemService(Context.WIFI_SERVICE) as WifiManager
            discoveryMulticastLock = wifi.createMulticastLock(DISCOVERY_MULTICAST_LOCK_TAG).apply {
                setReferenceCounted(false)
                acquire()
            }
            invoke.resolve()
        } catch (_: Exception) {
            releaseDiscoveryLock()
            invoke.reject("Android discovery multicast lock could not be acquired")
        }
    }

    @Command
    fun releaseDiscoveryMulticastLock(invoke: Invoke) {
        releaseDiscoveryLock()
        invoke.resolve()
    }

    private fun releaseDiscoveryLock() {
        val lock = discoveryMulticastLock
        discoveryMulticastLock = null
        if (lock?.isHeld == true) {
            try {
                lock.release()
            } catch (_: RuntimeException) {
                // Process teardown also releases the lock; no recovery is needed here.
            }
        }
    }

    private companion object {
        const val NOTIFICATION_PERMISSION_REQUEST = 8787
        const val DISCOVERY_MULTICAST_LOCK_TAG = "nekodex-mdns-discovery"
    }
}
