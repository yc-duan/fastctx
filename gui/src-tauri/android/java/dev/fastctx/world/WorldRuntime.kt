package dev.fastctx.world

import android.app.Activity
import android.app.NotificationManager
import android.content.Context
import android.content.Intent
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.PowerManager
import android.provider.Settings
import androidx.core.app.NotificationManagerCompat
import org.json.JSONArray
import org.json.JSONObject
import java.lang.ref.WeakReference

object WorldRuntime {
    init { System.loadLibrary("fastctx_gui_lib") }
    lateinit var context: Context
        private set
    lateinit var network: WorldNetwork
        private set
    lateinit var notices: WorldNotices
        private set
    private var started = false
    @Volatile var focused = false
    @Volatile var currentActivity = WeakReference<Activity>(null)
    @Volatile var serviceRunning = false
    @Volatile private var joining = false
    @Volatile var serviceError: String? = null
    @Volatile var observerError: String? = null
    @Volatile var connectionError: String? = null
    @Volatile var connection = JSONObject().put("hubs", JSONArray())
    private val main = Handler(Looper.getMainLooper())

    private external fun startNative(directory: String): String?
    private external fun reconnectNative()
    external fun activityNative(ready: Boolean)
    external fun visibleNative(visible: Boolean)
    external fun wakePageNative()
    external fun validateConfigurationNative(json: String): String?

    @Synchronized fun ensure(context: Context) {
        if (started) return
        this.context = context.applicationContext
        notices = WorldNotices(this.context)
        network = WorldNetwork(this.context)
        val error = startNative(this.context.noBackupFilesDir.resolve("world").absolutePath)
        if (error != null) throw IllegalStateException(error)
        // Started only once the network is followed too: a failure here is tried again whole.
        network.start()
        started = true
    }

    fun reconnect() { if (started) reconnectNative() }
    fun networkChoices(target: String) = network.choices(target)
    fun configuration(): String = context.getSharedPreferences("world", Context.MODE_PRIVATE)
        .getString("notificationPolicy", "{\"enabled\":false,\"rules\":[]}")!!
    fun isFocused() = focused
    fun showNotice(json: String) = notices.show(JSONObject(json))
    fun dismissNotice(id: String) = notices.dismiss(id)
    fun reportError(error: String) {
        observerError = error.ifEmpty { null }
        if (error.isNotEmpty()) android.util.Log.e("World", error)
    }
    fun reportConnectionError(error: String) {
        connectionError = error.ifEmpty { null }
        if (error.isNotEmpty()) android.util.Log.e("World", error)
    }
    fun setJoining(value: String) {
        joining = value == "true"
        if (joining) main.post { startService() }
    }

    /** How the phone's links to the hubs it is logged in to stand, in one word: "online" when any
     *  is, "stopped" when every one was stopped by its hub or there is none, else "connecting". */
    fun linkKind(state: JSONObject): String {
        val hubs = state.optJSONArray("hubs") ?: return "stopped"
        val kinds = (0 until hubs.length()).map { hubs.getJSONObject(it).getJSONObject("link").getString("kind") }
        return when {
            kinds.all { it == "stopped" } -> "stopped"
            kinds.any { it == "online" } -> "online"
            else -> "connecting"
        }
    }

    fun publishState(json: String) {
        val state = JSONObject(json)
        connection = state
        val joined = !state.isNull("name")
        val running = joining || (joined && linkKind(state) != "stopped")
        val preferences = context.getSharedPreferences("world", Context.MODE_PRIVATE)
        if (preferences.getBoolean("keepConnected", false) != running && !preferences.edit().putBoolean("keepConnected", running).commit())
            throw IllegalStateException("Could not save World background connection preference")
        main.post {
            if (running && !serviceRunning && focused && serviceError == null) startService()
            if (!running && serviceRunning) context.stopService(Intent(context, WorldService::class.java))
            if (serviceRunning) notices.connection(state)
        }
    }

    fun startService() {
        try {
            val intent = Intent(context, WorldService::class.java)
            if (Build.VERSION.SDK_INT >= 26) context.startForegroundService(intent) else context.startService(intent)
            serviceError = null
        } catch (error: Exception) {
            serviceError = error.toString()
            android.util.Log.e("World", "Starting background connection", error)
        }
    }

    fun resume(activity: Activity) {
        currentActivity = WeakReference(activity)
        focused = activity.hasWindowFocus()
        if (!serviceRunning && !connection.isNull("name") && linkKind(connection) != "stopped") startService()
    }

    fun status(): JSONObject {
        val power = context.getSystemService(PowerManager::class.java)
        val manager = context.getSystemService(NotificationManager::class.java)
        val alerts = Build.VERSION.SDK_INT < 26 || manager.getNotificationChannel(WorldNotices.ALERTS)?.importance != NotificationManager.IMPORTANCE_NONE
        return JSONObject().put("connection", connection).put("serviceRunning", serviceRunning)
            .put("serviceError", serviceError ?: JSONObject.NULL)
            .put("observerError", observerError ?: JSONObject.NULL)
            .put("connectionError", connectionError ?: JSONObject.NULL)
            .put("notificationsAllowed", NotificationManagerCompat.from(context).areNotificationsEnabled())
            .put("alertChannelAllowed", alerts)
            .put("policyConfigured", context.getSharedPreferences("world", Context.MODE_PRIVATE).contains("notificationPolicy"))
            .put("batteryOptimizationExempt", power.isIgnoringBatteryOptimizations(context.packageName))
            .put("powerSave", power.isPowerSaveMode).put("deviceIdle", power.isDeviceIdleMode)
            .put("lowPowerStandby", if (Build.VERSION.SDK_INT >= 33) power.isLowPowerStandbyEnabled else false)
            .put("lowPowerStandbyExempt", if (Build.VERSION.SDK_INT >= 34) power.isExemptFromLowPowerStandby else JSONObject.NULL)
            .put("manufacturer", Build.MANUFACTURER).put("model", Build.MODEL).put("sdk", Build.VERSION.SDK_INT)
            .put("deviceName", deviceName())
            .put("background", WorldBackground.details(context))
    }

    /** What the phone is called in its own settings, as its owner sees it; its maker and model, which
     *  can be a code, where it has no such name. */
    private fun deviceName(): String =
        (if (Build.VERSION.SDK_INT >= 25) Settings.Global.getString(context.contentResolver, Settings.Global.DEVICE_NAME) else null)
            ?.takeIf { it.isNotBlank() } ?: "${Build.MANUFACTURER} ${Build.MODEL}"
}
