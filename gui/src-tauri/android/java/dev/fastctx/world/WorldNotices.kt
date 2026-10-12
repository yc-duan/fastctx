package dev.fastctx.world

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.BroadcastReceiver
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
        const val NOTICE = "world_notice"
        const val ANSWER_NOTICE = "world_answer_notice"
        const val ANSWER_CHOICE = "world_answer_choice"
    }
    private val manager = context.getSystemService(NotificationManager::class.java)
    private val saved = context.getSharedPreferences("world-notices", Context.MODE_PRIVATE)

    init {
        if (Build.VERSION.SDK_INT >= 26) {
            manager.createNotificationChannel(NotificationChannel(CONNECTION, "World connection", NotificationManager.IMPORTANCE_LOW))
            manager.createNotificationChannel(NotificationChannel(ALERTS, "World conversations", NotificationManager.IMPORTANCE_HIGH))
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

    /** A button giving one of a question's answers, sent from the notification without opening the app. */
    private fun answerIntent(tag: String, choice: Int): PendingIntent {
        val intent = Intent(context, WorldAnswer::class.java).apply {
            data = Uri.Builder().scheme("world-answer").authority(context.packageName).appendPath(tag).appendPath(choice.toString()).build()
            putExtra(ANSWER_NOTICE, tag)
            putExtra(ANSWER_CHOICE, choice)
        }
        return PendingIntent.getBroadcast(context, 0, intent, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
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
        if (Build.VERSION.SDK_INT >= 26) check(manager.getNotificationChannel(ALERTS).importance != NotificationManager.IMPORTANCE_NONE) { "The World conversations notification channel is disabled" }
        val route = notice.getString("route")
        require(route.startsWith("#/talk/")) { "A conversation notification requires a #/talk/ route" }
        val tag = notice.getString("id")
        val previous = saved.getString("notice:$tag", null)?.let(::JSONObject)
        val activation = previous?.getString("activation") ?: UUID.randomUUID().toString()
        notice.put("activation", activation)
        check(saved.edit().putString("notice:$tag", notice.toString()).putString("activation:$activation", tag).commit()) { "Could not save notification destination" }
        val builder = NotificationCompat.Builder(context, ALERTS).setSmallIcon(R.drawable.world_notification)
            .setContentTitle(notice.getString("title")).setContentText(notice.getString("body"))
            .setStyle(NotificationCompat.BigTextStyle().bigText(notice.getString("body")))
            .setContentIntent(launch(activation)).setAutoCancel(true)
            .setOnlyAlertOnce(previous != null && previous.optString("version") == notice.optString("version"))
            .setPriority(NotificationCompat.PRIORITY_HIGH).setVisibility(NotificationCompat.VISIBILITY_PRIVATE)
        val answers = notice.optJSONArray("answers") ?: JSONArray()
        for (choice in 0 until answers.length()) builder.addAction(0, answers.getJSONObject(choice).getString("label"), answerIntent(tag, choice))
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

    /** Sends the answer a notification's button gives, as the user's words to its conversation; the
     *  notification goes once they are sent, and stays, with the failure reported, when they could not be. */
    fun answer(tag: String, choice: Int) {
        val notice = synchronized(this) { saved.getString("notice:$tag", null)?.let(::JSONObject) } ?: return
        val words = notice.getJSONArray("answers").getJSONObject(choice).getString("words")
        val to = notice.getJSONObject("to")
        // The same button pressed again after a failure that did send is the same words.
        val error = WorldRuntime.sayNative(to.getString("resident"), to.getString("machine"), words, "notice:$tag:$choice")
        check(error == null) { error!! }
        dismiss(tag)
    }

    @Synchronized fun pending() = JSONObject().put("deliveries", JSONArray(saved.getString("pending", "[]")))

    @Synchronized fun acknowledge(id: String) {
        val pending = JSONArray(saved.getString("pending", "[]"))
        check(pending.length() > 0 && pending.getJSONObject(0).getString("id") == id) { "Notification activations must be acknowledged in order" }
        pending.remove(0)
        check(saved.edit().putString("pending", pending.toString()).commit()) { "Could not acknowledge notification activation" }
    }
}

/** A notification's answer button: the answer goes out in the background, the app stays closed. */
class WorldAnswer : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val tag = intent.getStringExtra(WorldNotices.ANSWER_NOTICE) ?: return
        val choice = intent.getIntExtra(WorldNotices.ANSWER_CHOICE, -1)
        WorldRuntime.ensure(context)
        val pending = goAsync()
        Thread {
            try {
                WorldRuntime.notices.answer(tag, choice)
            } catch (error: Exception) {
                WorldRuntime.reportError("answering from a notification: $error")
            } finally {
                pending.finish()
            }
        }.start()
    }
}
