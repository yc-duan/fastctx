package dev.fastctx.pages

import android.app.Activity
import android.net.Uri
import android.os.Message
import android.view.View
import android.view.ViewGroup
import android.webkit.*
import android.widget.FrameLayout
import androidx.webkit.JavaScriptReplyProxy
import androidx.webkit.WebViewCompat
import androidx.webkit.WebViewFeature
import androidx.appcompat.app.AppCompatActivity
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Channel
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import java.io.ByteArrayInputStream
import java.util.concurrent.atomic.AtomicBoolean
import kotlin.math.roundToInt

@InvokeArg
class Bounds {
    var x = 0.0
    var y = 0.0
    var width = 0.0
    var height = 0.0
    var scale = 1.0
    var visible = false
}

@InvokeArg
class MountArgs {
    lateinit var id: String
    lateinit var html: String
    lateinit var wrapper: String
    lateinit var policy: String
    lateinit var wrapperPolicy: String
    lateinit var navigation: String
    lateinit var bounds: Bounds
    lateinit var messages: Channel
}

@InvokeArg
class SurfaceArgs {
    lateinit var id: String
    var bounds: Bounds? = null
    var message: String? = null
}

private class Surface(val view: WebView, val messages: Channel) {
    var reply: JavaScriptReplyProxy? = null
}

@TauriPlugin
class PageSandboxPlugin(private val activity: Activity) : Plugin(activity) {
    private val surfaces = mutableMapOf<String, Surface>()
    private lateinit var host: WebView
    private val origin = "https://world-page.invalid"
    private val wrapperUrl = "$origin/host"
    private val contentUrl = "$origin/content"

    override fun load(webView: WebView) { host = webView }

    override fun onDestroy(activity: AppCompatActivity) {
        surfaces.values.forEach {
            (it.view.parent as? ViewGroup)?.removeView(it.view)
            it.view.destroy()
        }
        surfaces.clear()
    }

    private fun failure(surface: Surface, reason: String) {
        surface.view.visibility = View.GONE
        surface.messages.send(JSObject().put("kind", "failed").put("value", reason) as JSObject)
    }

    private fun position(surface: Surface, bounds: Bounds) {
        val parent = host.parent as ViewGroup
        val hostLocation = IntArray(2)
        val parentLocation = IntArray(2)
        host.getLocationInWindow(hostLocation)
        parent.getLocationInWindow(parentLocation)
        surface.view.layoutParams = FrameLayout.LayoutParams(
            (bounds.width * bounds.scale).roundToInt(), (bounds.height * bounds.scale).roundToInt()
        ).apply {
            leftMargin = hostLocation[0] - parentLocation[0] + (bounds.x * bounds.scale).roundToInt()
            topMargin = hostLocation[1] - parentLocation[1] + (bounds.y * bounds.scale).roundToInt()
        }
        surface.view.visibility = if (bounds.visible) View.VISIBLE else View.GONE
    }

    @Command
    fun mount(invoke: Invoke) {
        try {
            val args = invoke.parseArgs(MountArgs::class.java)
            check(!surfaces.containsKey(args.id)) { "page surface already exists" }
            check(WebViewFeature.isFeatureSupported(WebViewFeature.WEB_MESSAGE_LISTENER)) { "this WebView does not support origin-bound page messages" }
            check(WebViewFeature.isFeatureSupported(WebViewFeature.DOCUMENT_START_SCRIPT)) { "this WebView does not support page navigation isolation" }
            val parent = host.parent
            check(parent is FrameLayout) { "the interface WebView must have a FrameLayout parent" }
            // A plain WebView deliberately never enters wry's injection path.
            val view = WebView(activity)
            val surface = Surface(view, args.messages)
            view.settings.apply {
                javaScriptEnabled = true
                domStorageEnabled = false
                databaseEnabled = false
                allowFileAccess = false
                allowContentAccess = false
                allowFileAccessFromFileURLs = false
                allowUniversalAccessFromFileURLs = false
                blockNetworkLoads = true
                blockNetworkImage = true
                cacheMode = WebSettings.LOAD_NO_CACHE
                javaScriptCanOpenWindowsAutomatically = false
                setSupportMultipleWindows(true)
                setGeolocationEnabled(false)
                mediaPlaybackRequiresUserGesture = true
                mixedContentMode = WebSettings.MIXED_CONTENT_NEVER_ALLOW
            }
            CookieManager.getInstance().setAcceptThirdPartyCookies(view, false)
            view.isLongClickable = false
            view.setOnLongClickListener { true }
            view.setDownloadListener { _, _, _, _, _ -> }
            view.webChromeClient = object : WebChromeClient() {
                override fun onCreateWindow(v: WebView, dialog: Boolean, gesture: Boolean, result: Message) = false
                override fun onPermissionRequest(request: PermissionRequest) { request.deny() }
                override fun onJsAlert(v: WebView, url: String, message: String, result: JsResult): Boolean { result.cancel(); return true }
                override fun onJsConfirm(v: WebView, url: String, message: String, result: JsResult): Boolean { result.cancel(); return true }
                override fun onJsPrompt(v: WebView, url: String, message: String, default: String, result: JsPromptResult): Boolean { result.cancel(); return true }
                override fun onShowFileChooser(v: WebView, callback: android.webkit.ValueCallback<Array<Uri>>, params: FileChooserParams): Boolean { callback.onReceiveValue(null); return true }
            }
            val hostServed = AtomicBoolean(false)
            val pageServed = AtomicBoolean(false)
            val pageNavigated = AtomicBoolean(false)
            view.webViewClient = object : WebViewClient() {
                override fun shouldOverrideUrlLoading(v: WebView, request: WebResourceRequest): Boolean {
                    return !(request.url.toString() == contentUrl && !request.isForMainFrame && !pageNavigated.getAndSet(true))
                }
                override fun shouldInterceptRequest(v: WebView, request: WebResourceRequest): WebResourceResponse {
                    val url = request.url.toString()
                    val (body, policy, code) = if (request.method == "GET" && url == "https://world-page-check.invalid/check") {
                        Triple("canary reached native loader", "default-src 'none'", 200)
                    } else if (request.method == "GET" && request.isForMainFrame && url == wrapperUrl && !hostServed.getAndSet(true)) {
                        Triple(args.wrapper, args.wrapperPolicy, 200)
                    } else if (request.method == "GET" && !request.isForMainFrame && url == contentUrl && !pageServed.getAndSet(true)) {
                        Triple(args.html, args.policy, 200)
                    } else Triple("", "default-src 'none'", 403)
                    return WebResourceResponse("text/html", "utf-8", code, "World page", mapOf(
                        "Content-Security-Policy" to policy, "Cache-Control" to "no-store",
                        "Connection-Allowlist" to (if (url == wrapperUrl) "(response-origin)" else "()"),
                        "Access-Control-Allow-Origin" to "*",
                        "X-DNS-Prefetch-Control" to "off", "Referrer-Policy" to "no-referrer"
                    ), ByteArrayInputStream(body.toByteArray(Charsets.UTF_8)))
                }
                override fun onReceivedSslError(v: WebView, handler: SslErrorHandler, error: android.net.http.SslError) { handler.cancel() }
                override fun onRenderProcessGone(v: WebView, detail: RenderProcessGoneDetail): Boolean {
                    failure(surface, "the page's browser process stopped")
                    (v.parent as? ViewGroup)?.removeView(v)
                    v.destroy()
                    surfaces.remove(args.id)
                    return true
                }
            }
            WebViewCompat.addWebMessageListener(view, "worldPageNative", setOf(origin)) { _, message, source, main, reply ->
                if (main && source.toString() == origin && message.data != null) {
                    surface.reply = reply
                    val value = JSObject(message.data!!)
                    if (value.optString("jsonrpc") == "2.0") args.messages.send(JSObject().put("kind", "message").put("value", value) as JSObject)
                }
            }
            WebViewCompat.addDocumentStartJavaScript(view, args.navigation, setOf("*"))
            surfaces[args.id] = surface
            parent.addView(view)
            position(surface, args.bounds)
            view.loadUrl(wrapperUrl)
            invoke.resolve()
        } catch (error: Exception) { invoke.reject(error.message) }
    }

    @Command
    fun place(invoke: Invoke) {
        try {
            val args = invoke.parseArgs(SurfaceArgs::class.java)
            position(surfaces[args.id] ?: error("page surface is closed"), args.bounds ?: error("missing bounds"))
            invoke.resolve()
        } catch (error: Exception) { invoke.reject(error.message) }
    }

    @Command
    fun post(invoke: Invoke) {
        try {
            val args = invoke.parseArgs(SurfaceArgs::class.java)
            val surface = surfaces[args.id] ?: error("page surface is closed")
            (surface.reply ?: error("page bridge has not connected")).postMessage(args.message ?: error("missing message"))
            invoke.resolve()
        } catch (error: Exception) { invoke.reject(error.message) }
    }

    @Command
    fun close(invoke: Invoke) {
        try {
            val args = invoke.parseArgs(SurfaceArgs::class.java)
            surfaces.remove(args.id)?.let {
                (it.view.parent as? ViewGroup)?.removeView(it.view)
                it.view.destroy()
            }
            invoke.resolve()
        } catch (error: Exception) { invoke.reject(error.message) }
    }
}
