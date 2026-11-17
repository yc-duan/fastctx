package dev.fastctx.world

import android.app.ActivityManager
import android.app.ApplicationExitInfo
import android.app.NotificationManager
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.net.ConnectivityManager
import android.os.Build
import org.json.JSONArray
import org.json.JSONObject
import java.util.Locale

object WorldBackground {
    private fun vendor(): String {
        val brands = listOf(Build.MANUFACTURER, Build.BRAND).map { it.lowercase(Locale.ROOT) }
        return when {
            brands.any { it == "honor" } -> "honor"
            brands.any { it == "huawei" } -> "huawei"
            brands.any { it == "xiaomi" || it == "redmi" || it == "poco" } -> "xiaomi"
            else -> "other"
        }
    }

    private fun component(packageName: String, className: String) = Intent().setComponent(ComponentName(packageName, className))
        .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)

    /** The maker's page where an app is let start by itself, tried in turn: the names these pages
     *  have had on the makers' recent systems. None for a maker without one. */
    fun autostart(context: Context): List<Intent> = when (vendor()) {
        "xiaomi" -> listOf(
            component("com.miui.securitycenter", "com.miui.permcenter.autostart.AutoStartManagementActivity"),
            component("com.miui.securitycenter", "com.miui.permcenter.permissions.PermissionsEditorActivity")
                .putExtra("extra_pkgname", context.packageName))
        "honor" -> listOf(component("com.hihonor.systemmanager", "com.hihonor.systemmanager.startupmgr.ui.StartupNormalAppListActivity"))
        "huawei" -> listOf(component("com.huawei.systemmanager", "com.huawei.systemmanager.startupmgr.ui.StartupNormalAppListActivity"))
        else -> emptyList()
    }

    /** The maker's own battery page for the app, where it is let run without restriction. */
    fun vendorBattery(context: Context): List<Intent> = when (vendor()) {
        "xiaomi" -> listOf(component("com.miui.powerkeeper", "com.miui.powerkeeper.ui.HiddenAppsConfigActivity")
            .putExtra("package_name", context.packageName).putExtra("package_label", "Fastctx"))
        else -> emptyList()
    }

    /** How the app's last run ended, as Android kept it: whether the system ended it while it was
     *  meant to keep its connection, which a maker's background rules do. */
    private fun lastExit(context: Context): JSONObject? {
        if (Build.VERSION.SDK_INT < 30) return null
        val exit = context.getSystemService(ActivityManager::class.java)
            .getHistoricalProcessExitReasons(context.packageName, 0, 1).firstOrNull() ?: return null
        val byUser = exit.reason == ApplicationExitInfo.REASON_USER_REQUESTED || exit.reason == ApplicationExitInfo.REASON_USER_STOPPED
            || exit.reason == ApplicationExitInfo.REASON_EXIT_SELF || exit.reason == ApplicationExitInfo.REASON_PACKAGE_UPDATED
        return JSONObject().put("reason", exit.reason).put("at", exit.timestamp).put("byUser", byUser)
            .put("description", exit.description ?: JSONObject.NULL)
    }

    fun details(context: Context): JSONObject {
        val vendor = vendor()
        val sources = when (vendor) {
            "huawei" -> emptyList()
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
            .put("hasAutostart", autostart(context).isNotEmpty())
            .put("lastExit", lastExit(context) ?: JSONObject.NULL)
    }
}
