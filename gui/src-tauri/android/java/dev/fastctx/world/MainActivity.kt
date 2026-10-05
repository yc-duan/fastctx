package dev.fastctx.world

import android.content.Intent
import android.os.Bundle
import android.view.View
import android.view.ViewGroup
import android.widget.FrameLayout
import androidx.activity.OnBackPressedCallback
import androidx.activity.enableEdgeToEdge
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat

class MainActivity : TauriActivity() {
    override val handleBackNavigation: Boolean = true
    override fun onCreate(savedInstanceState: Bundle?) {
        WorldRuntime.ensure(applicationContext)
        enableEdgeToEdge()
        super.onCreate(savedInstanceState)
        WorldRuntime.currentActivity = java.lang.ref.WeakReference(this)
        WorldRuntime.activityNative(true)
        WorldRuntime.notices.activate(intent)
        onBackPressedDispatcher.addCallback(this, object : OnBackPressedCallback(true) {
            override fun handleOnBackPressed() { moveTaskToBack(true) }
        })
    }

    override fun setContentView(view: View?) {
        if (view == null) return
        // Padding a WebView does not inset its document viewport. Inset its
        // parent instead, including the keyboard, without changing page CSS.
        val container = FrameLayout(this)
        container.addView(view, FrameLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT))
        super.setContentView(container)
        ViewCompat.setOnApplyWindowInsetsListener(container) { content, insets ->
            val bars = insets.getInsets(WindowInsetsCompat.Type.systemBars() or WindowInsetsCompat.Type.displayCutout() or WindowInsetsCompat.Type.ime())
            content.setPadding(bars.left, bars.top, bars.right, bars.bottom)
            insets
        }
        ViewCompat.requestApplyInsets(container)
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
