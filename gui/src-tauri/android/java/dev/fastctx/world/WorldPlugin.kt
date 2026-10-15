package dev.fastctx.world

import android.Manifest
import android.app.Activity
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.provider.Settings
import android.webkit.MimeTypeMap
import androidx.core.content.FileProvider
import java.io.File
import androidx.activity.result.ActivityResult
import app.tauri.annotation.ActivityCallback
import app.tauri.annotation.Command
import app.tauri.annotation.Permission
import app.tauri.annotation.PermissionCallback
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import com.journeyapps.barcodescanner.ScanIntentResult
import com.journeyapps.barcodescanner.ScanOptions

@TauriPlugin(permissions = [Permission(alias = "notifications", strings = [Manifest.permission.POST_NOTIFICATIONS])])
class WorldPlugin(activity: Activity) : Plugin(activity) {
    private var selecting = false
    private var saving: Saving.Request? = null
    private fun current(): Activity = WorldRuntime.currentActivity.get()
        ?: throw IllegalStateException("World has no foreground activity")
    private fun answer(invoke: Invoke, work: () -> Unit) {
        try { work() } catch (error: Exception) { invoke.reject(error.toString()) }
    }

    @Command fun status(invoke: Invoke) = answer(invoke) { invoke.resolve(JSObject(WorldRuntime.status().toString())) }
    @Command fun openFile(invoke: Invoke) = answer(invoke) {
        val context = current()
        val file = File(invoke.getArgs().getString("path")).canonicalFile
        require(file.isFile && file.path.startsWith(context.cacheDir.canonicalPath + File.separator)) {
            "The file must be in World's cache"
        }
        val uri = FileProvider.getUriForFile(context, "${context.packageName}.fileprovider", file)
        val mime = MimeTypeMap.getSingleton().getMimeTypeFromExtension(file.extension.lowercase())
            ?: "application/octet-stream"
        context.startActivity(Intent(Intent.ACTION_VIEW).setDataAndType(uri, mime)
            .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION))
        invoke.resolve()
    }
    /** Hands a new version's package, checked by the World, to the system's installer. Without the
     *  user's leave to install apps, the system's page for giving it opens instead. */
    @Command fun installPackage(invoke: Invoke) = answer(invoke) {
        val context = current()
        val result = JSObject()
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O && !context.packageManager.canRequestPackageInstalls()) {
            context.startActivity(Intent(Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES,
                Uri.parse("package:${context.packageName}")))
            result.put("started", false)
            invoke.resolve(result)
            return@answer
        }
        val source = File(invoke.getArgs().getString("path")).canonicalFile
        val folder = File(context.cacheDir, "updates").apply { mkdirs() }
        val apk = File(folder, "Fastctx.apk")
        source.copyTo(apk, overwrite = true)
        val uri = FileProvider.getUriForFile(context, "${context.packageName}.fileprovider", apk)
        context.startActivity(Intent(Intent.ACTION_VIEW).setDataAndType(uri, "application/vnd.android.package-archive")
            .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_ACTIVITY_NEW_TASK))
        result.put("started", true)
        invoke.resolve(result)
    }
    @Command fun configure(invoke: Invoke) = answer(invoke) {
        val configuration = invoke.getArgs().getJSONObject("configuration").toString()
        val error = WorldRuntime.validateConfigurationNative(configuration)
        require(error == null) { error!! }
        check(WorldRuntime.context.getSharedPreferences("world", Activity.MODE_PRIVATE).edit().putString("notificationPolicy", configuration).commit()) { "Could not keep notification policy" }
        invoke.resolve()
    }
    @Command fun pending(invoke: Invoke) = answer(invoke) { invoke.resolve(JSObject(WorldRuntime.notices.pending().toString())) }
    @Command fun acknowledge(invoke: Invoke) = answer(invoke) {
        WorldRuntime.notices.acknowledge(invoke.getArgs().getString("id")); invoke.resolve()
    }
    @Command fun notify(invoke: Invoke) = answer(invoke) {
        val id = WorldRuntime.notices.show(invoke.getArgs().getJSONObject("notice"))
        invoke.resolve(JSObject().put("id", id))
    }
    @Command fun dismiss(invoke: Invoke) = answer(invoke) {
        WorldRuntime.notices.dismiss(invoke.getArgs().getString("id")); invoke.resolve()
    }
    @Command fun requestNotifications(invoke: Invoke) = answer(invoke) {
        if (Build.VERSION.SDK_INT >= 33) requestPermissionForAlias("notifications", invoke, "notificationPermission")
        else status(invoke)
    }
    @PermissionCallback fun notificationPermission(invoke: Invoke) = status(invoke)

    @Command fun settings(invoke: Invoke) = answer(invoke) {
        val context = current()
        val destination = invoke.getArgs().getString("destination")
        val intent = when (destination) {
            "application" -> Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.parse("package:${context.packageName}"))
            "battery" -> Intent(Settings.ACTION_IGNORE_BATTERY_OPTIMIZATION_SETTINGS)
            "notifications" -> if (Build.VERSION.SDK_INT >= 26)
                Intent(Settings.ACTION_APP_NOTIFICATION_SETTINGS).putExtra(Settings.EXTRA_APP_PACKAGE, context.packageName)
                else Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.parse("package:${context.packageName}"))
            else -> throw IllegalArgumentException("Unknown settings destination: $destination")
        }
        context.startActivity(intent)
        invoke.resolve()
    }

    @Command fun scan(invoke: Invoke) = answer(invoke) {
        check(!selecting) { "A system picker is already open" }
        val context = current()
        check(context.packageManager.hasSystemFeature(PackageManager.FEATURE_CAMERA_ANY)) { "This device has no camera" }
        val options = ScanOptions().setDesiredBarcodeFormats(ScanOptions.QR_CODE).setBeepEnabled(false)
            .setBarcodeImageEnabled(false).setOrientationLocked(false)
        selecting = true
        try { startActivityForResult(invoke, options.createScanIntent(context), "scanResult") }
        catch (error: Exception) { selecting = false; throw error }
    }
    @ActivityCallback fun scanResult(invoke: Invoke, result: ActivityResult) = answer(invoke) {
        selecting = false
        val parsed = ScanIntentResult.parseActivityResult(result.resultCode, result.data)
        if (parsed.contents != null) invoke.resolve(JSObject().put("kind", "scanned").put("code", parsed.contents))
        else if (result.data?.getBooleanExtra("MISSING_CAMERA_PERMISSION", false) == true)
            invoke.reject("Android camera permission was denied")
        else invoke.resolve(JSObject().put("kind", "cancelled"))
    }

    @Command fun saveAs(invoke: Invoke) {
        try {
            check(!selecting) { "A system picker is already open" }
            current()
            val request = Saving.request(invoke)
            saving = request
            selecting = true
            try { startActivityForResult(invoke, request.intent, "saveResult") }
            catch (error: Exception) { selecting = false; saving = null; request.close(); throw error }
        } catch (error: Exception) { Saving.failed(invoke, error) }
    }
    @ActivityCallback fun saveResult(invoke: Invoke, result: ActivityResult) {
        selecting = false
        val request = saving
        saving = null
        if (request == null) Saving.failed(invoke, IllegalStateException("The save request is gone"))
        else try { Saving.complete(invoke, request, result) } catch (error: Exception) { Saving.failed(invoke, error) }
    }
}
