package dev.fastctx.world

import android.content.Intent
import android.os.Bundle
import android.view.View
import android.webkit.WebView
import androidx.activity.OnBackPressedCallback
import java.lang.ref.WeakReference

class MainActivity : TauriActivity() {
    // Back is World's own (below), not the WebView's history alone.
    override val handleBackNavigation: Boolean = false
    private lateinit var frame: WorldFrame
    private var webView = WeakReference<WebView>(null)
    private val back = object : OnBackPressedCallback(true) {
        override fun handleOnBackPressed() {
            val view = webView.get()
            if (view == null) {
                moveTaskToBack(true)
                return
            }
            view.evaluateJavascript("typeof __worldBack === 'function' && __worldBack()") { closed ->
                if (closed == "true") return@evaluateJavascript
                if (view.canGoBack()) view.goBack() else moveTaskToBack(true)
            }
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        WorldRuntime.ensure(applicationContext)
        frame = WorldFrame(this, ::armBack)
        frame.begin()
        super.onCreate(savedInstanceState)
        WorldRuntime.currentActivity = java.lang.ref.WeakReference(this)
        WorldRuntime.activityNative(true)
        WorldRuntime.notices.activate(intent)
        // The system back, as a phone's user expects it: the page closes what lies over it,
        // topmost first; with nothing open it goes back a page; on the first page the app steps
        // aside, keeping the World running.
        armBack()
    }

    override fun onWebViewCreate(webView: WebView) {
        this.webView = WeakReference(webView)
        frame.attach(webView)
    }

    private fun armBack() {
        // Tauri's app plugin installs its history callback after onCreate.
        // The ready page puts World's callback last, so it receives back first.
        back.remove()
        onBackPressedDispatcher.addCallback(this, back)
    }

    override fun setContentView(view: View?) {
        if (view == null) return
        super.setContentView(frame.content(view))
    }

    override fun onResume() { super.onResume(); WorldRuntime.resume(this); WorldRuntime.visibleNative(true) }
    override fun onPause() {
        if (WorldRuntime.currentActivity.get() === this) {
            WorldRuntime.focused = false
            WorldRuntime.visibleNative(false)
        }
        super.onPause()
    }
    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        if (WorldRuntime.currentActivity.get() === this) WorldRuntime.focused = hasFocus
    }
    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        WorldRuntime.notices.activate(intent)
    }
    override fun onDestroy() {
        if (WorldRuntime.currentActivity.get() === this) {
            WorldRuntime.activityNative(false)
            WorldRuntime.focused = false
            WorldRuntime.currentActivity.clear()
        }
        super.onDestroy()
    }
}
