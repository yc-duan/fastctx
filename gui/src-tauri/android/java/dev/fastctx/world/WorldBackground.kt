package dev.fastctx.world

import android.app.ActivityManager
import android.app.NotificationManager
import android.content.Context
import android.net.ConnectivityManager
import android.os.Build
import org.json.JSONArray
import org.json.JSONObject
import java.util.Locale

object WorldBackground {
    fun details(context: Context): JSONObject {
        val brands = listOf(Build.MANUFACTURER, Build.BRAND).map { it.lowercase(Locale.ROOT) }
        val vendor = when {
            brands.any { it == "honor" } -> "honor"
            brands.any { it == "xiaomi" || it == "redmi" || it == "poco" } -> "xiaomi"
            else -> "other"
        }
        val sources = when (vendor) {
            "honor" -> listOf("https://www.honor.com/cn/support/content/zh-cn00685514/",
                "https://www.honor.com/cn/support/content/zh-cn00406916/",
                "https://www.honor.com/cn/support/content/zh-cn15807802/")
            "xiaomi" -> listOf("https://dev.mi.com/xiaomihyperos/documentation/detail?pId=1624",
                "https://dev.mi.com/xiaomihyperos/documentation/detail?pId=1628",
                "https://www.mi.com/global/support/faq/details/KA-694584/")
            else -> emptyList()
        }
        val notifications = context.getSystemService(NotificationManager::class.java)
        val channel = if (Build.VERSION.SDK_INT >= 26) notifications.getNotificationChannel(WorldNotices.ALERTS) else null
        val connectivity = context.getSystemService(ConnectivityManager::class.java)
        return JSONObject().put("vendor", vendor).put("brand", Build.BRAND)
            .put("androidVersion", Build.VERSION.RELEASE).put("buildDisplay", Build.DISPLAY)
            .put("buildIncremental", Build.VERSION.INCREMENTAL)
            .put("backgroundRestricted", if (Build.VERSION.SDK_INT >= 28)
                context.getSystemService(ActivityManager::class.java).isBackgroundRestricted else JSONObject.NULL)
            .put("dataSaverStatus", connectivity.restrictBackgroundStatus)
            .put("interruptionFilter", notifications.currentInterruptionFilter)
            .put("alertImportance", channel?.importance ?: JSONObject.NULL)
            .put("alertLockscreenVisibility", channel?.lockscreenVisibility ?: JSONObject.NULL)
            // Public Android APIs do not expose either vendor's private
            // switches. A battery-optimization exemption does not prove them.
            .put("autostartAllowed", JSONObject.NULL).put("vendorBackgroundAllowed", JSONObject.NULL)
            .put("recentsLocked", JSONObject.NULL).put("sleepNetworkAllowed", JSONObject.NULL)
            .put("guidanceSources", JSONArray(sources))
    }
}
