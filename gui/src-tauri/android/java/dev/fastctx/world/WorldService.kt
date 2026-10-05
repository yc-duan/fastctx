package dev.fastctx.world

import android.app.Service
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import android.os.PowerManager

class WorldService : Service() {
    private var wake: PowerManager.WakeLock? = null
    private var receiverRegistered = false
    private var ready = false
    private val power = object : BroadcastReceiver() {
        override fun onReceive(context: Context, intent: Intent) = WorldRuntime.reconnect()
    }
    override fun onCreate() {
        super.onCreate()
        try {
        val notices = WorldNotices(applicationContext)
        if (Build.VERSION.SDK_INT >= 34) startForeground(WorldNotices.CONNECTION_ID, notices.connectionNotification(null), ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE)
        else startForeground(WorldNotices.CONNECTION_ID, notices.connectionNotification(null))
        WorldRuntime.ensure(applicationContext)
        WorldRuntime.serviceRunning = true
        wake = getSystemService(PowerManager::class.java).newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "World:connection").also {
            it.setReferenceCounted(false)
            it.acquire()
        }
        val filter = IntentFilter().apply {
            addAction(Intent.ACTION_SCREEN_ON)
            addAction(Intent.ACTION_USER_PRESENT)
            addAction(PowerManager.ACTION_DEVICE_IDLE_MODE_CHANGED)
            addAction(PowerManager.ACTION_POWER_SAVE_MODE_CHANGED)
            if (Build.VERSION.SDK_INT >= 33) addAction(PowerManager.ACTION_LOW_POWER_STANDBY_ENABLED_CHANGED)
        }
        if (Build.VERSION.SDK_INT >= 33) registerReceiver(power, filter, Context.RECEIVER_NOT_EXPORTED)
        else registerReceiver(power, filter)
        receiverRegistered = true
        ready = true
        } catch (error: Exception) {
            WorldRuntime.serviceRunning = false
            WorldRuntime.serviceError = error.toString()
            android.util.Log.e("World", "The background connection could not start", error)
            stopSelf()
        }
    }
    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (!ready) return START_NOT_STICKY
        return START_STICKY
    }
    override fun onDestroy() {
        WorldRuntime.serviceRunning = false
        if (receiverRegistered) unregisterReceiver(power)
        wake?.let { if (it.isHeld) it.release() }
        super.onDestroy()
    }
    override fun onBind(intent: Intent?): IBinder? = null
}

class WorldBoot : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action != Intent.ACTION_BOOT_COMPLETED && intent.action != Intent.ACTION_MY_PACKAGE_REPLACED) return
        if (!context.getSharedPreferences("world", Context.MODE_PRIVATE).getBoolean("keepConnected", false)) return
        val service = Intent(context, WorldService::class.java)
        if (Build.VERSION.SDK_INT >= 26) context.startForegroundService(service) else context.startService(service)
    }
}
