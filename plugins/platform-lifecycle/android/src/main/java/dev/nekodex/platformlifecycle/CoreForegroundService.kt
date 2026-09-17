package dev.nekodex.platformlifecycle

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.net.wifi.WifiManager
import android.os.IBinder
import androidx.core.app.NotificationCompat
import androidx.core.app.ServiceCompat

class CoreForegroundService : Service() {
    private var multicastLock: WifiManager.MulticastLock? = null

    override fun onCreate() {
        super.onCreate()
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (!coreAvailable) {
            stopSelf(startId)
            return START_NOT_STICKY
        }
        createNotificationChannel()
        ServiceCompat.startForeground(
            this,
            NOTIFICATION_ID,
            buildNotification(),
            ServiceInfo.FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE,
        )
        releaseMulticastLock()
        acquireMulticastLock()
        return START_STICKY
    }

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onDestroy() {
        releaseMulticastLock()
        ServiceCompat.stopForeground(this, ServiceCompat.STOP_FOREGROUND_REMOVE)
        super.onDestroy()
    }

    private fun createNotificationChannel() {
        val channel = NotificationChannel(
            CHANNEL_ID,
            getString(R.string.nekodex_connection_channel_name),
            NotificationManager.IMPORTANCE_LOW,
        ).apply {
            description = getString(R.string.nekodex_connection_channel_description)
            setShowBadge(false)
        }
        getSystemService(NotificationManager::class.java).createNotificationChannel(channel)
    }

    private fun buildNotification(): Notification {
        val launchIntent = packageManager.getLaunchIntentForPackage(packageName)
        val contentIntent = launchIntent?.let {
            PendingIntent.getActivity(
                this,
                0,
                it,
                PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
            )
        }
        val icon = applicationInfo.icon.takeIf { it != 0 } ?: android.R.drawable.stat_notify_sync
        return NotificationCompat.Builder(this, CHANNEL_ID)
            .setSmallIcon(icon)
            .setContentTitle(getString(R.string.nekodex_connection_title))
            .setContentText(getString(R.string.nekodex_connection_text))
            .setCategory(NotificationCompat.CATEGORY_SERVICE)
            .setPriority(NotificationCompat.PRIORITY_LOW)
            .setVisibility(NotificationCompat.VISIBILITY_PRIVATE)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setContentIntent(contentIntent)
            .build()
    }

    private fun acquireMulticastLock() {
        try {
            val wifi = applicationContext.getSystemService(Context.WIFI_SERVICE) as WifiManager
            multicastLock = wifi.createMulticastLock(MULTICAST_LOCK_TAG).apply {
                setReferenceCounted(false)
                acquire()
            }
        } catch (_: RuntimeException) {
            multicastLock = null
        }
    }

    private fun releaseMulticastLock() {
        val lock = multicastLock
        multicastLock = null
        if (lock?.isHeld == true) {
            try {
                lock.release()
            } catch (_: RuntimeException) {
                // The service is already stopping; there is nothing left to recover.
            }
        }
    }

    companion object {
        private const val CHANNEL_ID = "nekodex-core-connection"
        private const val NOTIFICATION_ID = 8787
        private const val MULTICAST_LOCK_TAG = "nekodex-mdns-service"
        @Volatile
        private var coreAvailable = false

        internal fun confirmCoreAvailable() {
            coreAvailable = true
        }

        internal fun confirmCoreUnavailable() {
            coreAvailable = false
        }
    }
}
