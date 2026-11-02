package dev.fastctx.world

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Build
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import org.json.JSONArray
import org.json.JSONObject
import java.util.UUID

class WorldNotices(private val context: Context) {
    companion object {
        const val CONNECTION_ID = 1
        const val CONNECTION = "world_connection"
        const val ALERTS = "world_conversations"
        const val MACHINES = "world_machines"
        const val NOTICE = "world_notice"
    }
    private val manager = context.getSystemService(NotificationManager::class.java)
    private val saved = context.getSharedPreferences("world-notices", Context.MODE_PRIVATE)

    init {
        if (Build.VERSION.SDK_INT >= 26) {
            manager.createNotificationChannel(NotificationChannel(CONNECTION, "World connection", NotificationManager.IMPORTANCE_LOW))
            manager.createNotificationChannel(NotificationChannel(ALERTS, "World conversations", NotificationManager.IMPORTANCE_HIGH))
            manager.createNotificationChannel(NotificationChannel(MACHINES, "World machines", NotificationManager.IMPORTANCE_DEFAULT))
        }
    }

    private fun launch(id: String?): PendingIntent {
        val intent = Intent(context, MainActivity::class.java).apply {
            flags = Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TOP or Intent.FLAG_ACTIVITY_SINGLE_TOP
            data = Uri.Builder().scheme("world-notice").authority(context.packageName).appendPath(id ?: "connection").build()
            if (id != null) putExtra(NOTICE, id)
        }
        return PendingIntent.getActivity(context, 0, intent, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
    }

    fun connectionNotification(state: JSONObject?): Notification {
        val kind = state?.let(WorldRuntime::linkKind)
        val text = when (kind) {
            "online" -> "Connected to your World"
            "stopped" -> "World connection stopped"
            else -> "Reconnecting to your World"
        }
        return NotificationCompat.Builder(context, CONNECTION).setSmallIcon(R.drawable.world_notification)
            .setContentTitle("Fastctx").setContentText(text).setOngoing(true).setOnlyAlertOnce(true)
            .setContentIntent(launch(null)).setCategory(NotificationCompat.CATEGORY_SERVICE).build()
    }

    fun connection(state: JSONObject) = manager.notify(CONNECTION_ID, connectionNotification(state))

    @Synchronized fun show(notice: JSONObject): String {
        check(NotificationManagerCompat.from(context).areNotificationsEnabled()) { "Android notifications are disabled for World" }
        val route = notice.getString("route")
        // A conversation's call goes on its own channel; what went wrong with
        // a machine or the network on the machines', which can be quieted
        // apart.
        val channel = when {
            route.startsWith("#/talk/") -> ALERTS
            route.startsWith("#/machine/") || route == "#/alerts" -> MACHINES
            else -> throw IllegalArgumentException("A notification requires a conversation's, a machine's or the alerts' route")
        }
        if (Build.VERSION.SDK_INT >= 26) check(manager.getNotificationChannel(channel).importance != NotificationManager.IMPORTANCE_NONE) { "The World notification channel $channel is disabled" }
        val tag = notice.getString("id")
        val previous = saved.getString("notice:$tag", null)?.let(::JSONObject)
        val activation = previous?.getString("activation") ?: UUID.randomUUID().toString()
        notice.put("activation", activation)
        check(saved.edit().putString("notice:$tag", notice.toString()).putString("activation:$activation", tag).commit()) { "Could not save notification destination" }
        val builder = NotificationCompat.Builder(context, channel).setSmallIcon(R.drawable.world_notification)
            .setContentTitle(notice.getString("title")).setContentText(notice.getString("body"))
            .setStyle(NotificationCompat.BigTextStyle().bigText(notice.getString("body")))
            .setContentIntent(launch(activation)).setAutoCancel(true)
            .setOnlyAlertOnce(previous != null && previous.optString("version") == notice.optString("version"))
            .setPriority(if (channel == ALERTS) NotificationCompat.PRIORITY_HIGH else NotificationCompat.PRIORITY_DEFAULT)
            .setVisibility(NotificationCompat.VISIBILITY_PRIVATE)
        manager.notify(tag, 0, builder.build())
        return tag
    }

    @Synchronized fun dismiss(id: String) {
        manager.cancel(id, 0)
        val value = saved.getString("notice:$id", null)?.let(::JSONObject) ?: return
        check(saved.edit().remove("notice:$id").remove("activation:${value.getString("activation")}").commit()) { "Could not forget notification destination" }
    }

    @Synchronized fun activate(intent: Intent) {
        val activation = intent.getStringExtra(NOTICE) ?: return
        intent.removeExtra(NOTICE)
        val tag = saved.getString("activation:$activation", null) ?: return
        val notice = saved.getString("notice:$tag", null)?.let(::JSONObject) ?: return
        val pending = JSONArray(saved.getString("pending", "[]"))
        val event = JSONObject().put("id", activation).put("route", notice.getString("route"))
            .put("action", "open").put("notificationId", tag)
        pending.put(event)
        check(saved.edit().putString("pending", pending.toString()).remove("activation:$activation").remove("notice:$tag").commit()) { "Could not keep notification activation" }
        manager.cancel(tag, 0)
        WorldRuntime.wakePageNative()
    }

    @Synchronized fun pending() = JSONObject().put("deliveries", JSONArray(saved.getString("pending", "[]")))

    @Synchronized fun acknowledge(id: String) {
        val pending = JSONArray(saved.getString("pending", "[]"))
        check(pending.length() > 0 && pending.getJSONObject(0).getString("id") == id) { "Notification activations must be acknowledged in order" }
        pending.remove(0)
        check(saved.edit().putString("pending", pending.toString()).commit()) { "Could not acknowledge notification activation" }
    }
}
