package dev.fastctx.world

import android.annotation.SuppressLint
import android.content.Context
import android.content.res.Configuration
import android.graphics.drawable.ColorDrawable
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.view.HapticFeedbackConstants
import android.view.View
import android.view.ViewGroup
import android.webkit.JavascriptInterface
import android.webkit.WebView
import android.widget.FrameLayout
import androidx.activity.ComponentActivity
import androidx.activity.SystemBarStyle
import androidx.activity.enableEdgeToEdge
import androidx.core.view.ViewCompat
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.graphics.ColorUtils
import java.lang.ref.WeakReference
import java.util.Locale

/**
 * World's window on a phone. The page runs edge to edge: its own background goes under the status
 * and navigation bars, and it keeps what matters clear of them with the `--safe-top` and
 * `--safe-bottom` lengths given here, the system's bar heights in CSS pixels. Sideways (a cut-out
 * or the buttons in landscape) and under the keyboard the page is inset instead, on a strip of
 * the current background's colour. The page names its theme, and whether what lies under the bars is light,
 * through the `WorldWindow` object; the choice is kept, so the next start paints the window,
 * the splash and the page's backdrop in it before the page has run.
 */
class WorldFrame(private val activity: ComponentActivity, private val pageReady: () -> Unit) {
    private val main = Handler(Looper.getMainLooper())
    private val preferences = activity.getSharedPreferences("world", Context.MODE_PRIVATE)
    private var container: FrameLayout? = null
    private var webView = WeakReference<WebView>(null)
    /** The theme painted, whether the user chose it, and whether the bars' icons sit on light. */
    private var theme = keptTheme()
    private var chosen = preferences.getBoolean(CHOSEN, false)
    private var lightUnder = theme == WARM_WHITE
    /** The page's safe lengths as JSON, read by the page as it starts on any thread. */
    @Volatile private var insets = "{\"top\":0,\"bottom\":0}"

    /** Before the activity's content: the bars go transparent over the page, in the kept theme. */
    fun begin() {
        activity.setTheme(if (chosen) splash(theme) else R.style.Theme_fastctx_gui)
        val transparent = android.graphics.Color.TRANSPARENT
        val style = if (lightUnder) SystemBarStyle.light(transparent, transparent) else SystemBarStyle.dark(transparent)
        activity.enableEdgeToEdge(style, style)
        activity.window.setBackgroundDrawable(ColorDrawable(colour(theme)))
        bars()
    }

    /** The WebView's parent: insets it sideways and above the keyboard, and gives the page the
     *  rest of the bars as lengths. A WebView's own padding does not inset its document. */
    fun content(view: View): View {
        val frame = FrameLayout(activity)
        frame.setBackgroundColor(colour(theme))
        frame.addView(view, FrameLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT))
        ViewCompat.setOnApplyWindowInsetsListener(frame) { content, all ->
            val edges = all.getInsets(WindowInsetsCompat.Type.systemBars() or WindowInsetsCompat.Type.displayCutout())
            content.setPadding(edges.left, 0, edges.right, all.getInsets(WindowInsetsCompat.Type.ime()).bottom)
            give(all)
            all
        }
        container = frame
        ViewCompat.requestApplyInsets(frame)
        return frame
    }

    /** The interface's WebView, as it is made and before it loads anything. */
    @SuppressLint("JavascriptInterface", "AddJavascriptInterface")
    fun attach(view: WebView) {
        webView = WeakReference(view)
        // Until the page paints, the WebView shows this, not its default white.
        view.setBackgroundColor(colour(theme))
        // The page reads the bars as it starts, which may be before its parent is laid out.
        ViewCompat.getRootWindowInsets(activity.window.decorView)?.let { give(it) }
        view.addJavascriptInterface(Bridge(), "WorldWindow")
    }

    private fun give(all: WindowInsetsCompat) {
        val edges = all.getInsets(WindowInsetsCompat.Type.systemBars() or WindowInsetsCompat.Type.displayCutout())
        val keyboard = all.getInsets(WindowInsetsCompat.Type.ime()).bottom
        val top = edges.top
        // With the keyboard up the page ends above it, so nothing of the page is under the
        // navigation bar any more.
        val bottom = (edges.bottom - keyboard).coerceAtLeast(0)
        val density = activity.resources.displayMetrics.density
        val css = { pixels: Int -> String.format(Locale.ROOT, "%.2f", pixels / density) }
        val (topCss, bottomCss) = css(top) to css(bottom)
        val said = "{\"top\":$topCss,\"bottom\":$bottomCss}"
        if (said == insets) return
        insets = said
        webView.get()?.evaluateJavascript(
            "document.documentElement.style.setProperty('--safe-top','${topCss}px');" +
                "document.documentElement.style.setProperty('--safe-bottom','${bottomCss}px');",
            null,
        )
    }

    private fun paint(name: String, byChoice: Boolean, light: Boolean, darkCover: Boolean) {
        // The picture viewer's background (conversation.css) also fills any
        // strips beside a landscape cut-out or the keyboard.
        val background = if (darkCover) ColorUtils.blendARGB(android.graphics.Color.rgb(13, 13, 12), colour(name), 0.18f) else colour(name)
        val changedTheme = name != theme || byChoice != chosen
        theme = name
        chosen = byChoice
        lightUnder = light
        activity.window.setBackgroundDrawable(ColorDrawable(background))
        container?.setBackgroundColor(background)
        webView.get()?.setBackgroundColor(background)
        bars()
        if (!changedTheme) return
        check(preferences.edit().putString(THEME, name).putBoolean(CHOSEN, byChoice).commit()) { "Could not keep the theme" }
        // The splash of the next start takes the chosen theme; following the system it takes the
        // system's light or dark from the application theme.
        if (Build.VERSION.SDK_INT >= 31) activity.splashScreen.setSplashScreenTheme(if (byChoice) splash(name) else R.style.Theme_fastctx_gui)
    }

    private fun bars() {
        val controller = WindowCompat.getInsetsController(activity.window, activity.window.decorView)
        controller.isAppearanceLightStatusBars = lightUnder
        controller.isAppearanceLightNavigationBars = lightUnder
    }

    /** Each theme's page background (`res/values/world_colors.xml`). */
    private fun colour(theme: String): Int = activity.getColor(
        when (theme) {
            WARM_BLACK -> R.color.world_warm_black
            INK_BLACK -> R.color.world_ink_black
            WARM_WHITE -> R.color.world_warm_white
            else -> throw IllegalArgumentException("Unknown theme: $theme")
        },
    )

    private fun keptTheme(): String =
        if (preferences.getBoolean(CHOSEN, false)) preferences.getString(THEME, null)?.takeIf { it in THEMES } ?: systemTheme()
        else systemTheme()

    private fun systemTheme(): String =
        if ((activity.resources.configuration.uiMode and Configuration.UI_MODE_NIGHT_MASK) == Configuration.UI_MODE_NIGHT_YES) WARM_BLACK
        else WARM_WHITE

    /** What the page calls; WebView calls it on its own thread. */
    private inner class Bridge {
        @JavascriptInterface fun insets(): String = this@WorldFrame.insets
        @JavascriptInterface fun ready() { main.post { pageReady() } }
        /** A long press the page answered: the knock the system gives its own, as the user set it. */
        @JavascriptInterface fun press() { main.post { webView.get()?.performHapticFeedback(HapticFeedbackConstants.LONG_PRESS) } }

        @JavascriptInterface fun paint(theme: String, chosen: Boolean, lightUnderBars: Boolean, darkCover: Boolean) {
            require(theme in THEMES) { "Unknown theme: $theme" }
            main.post { this@WorldFrame.paint(theme, chosen, lightUnderBars, darkCover) }
        }
    }

    companion object {
        private const val THEME = "theme"
        private const val CHOSEN = "themeChosen"
        private const val WARM_BLACK = "warm-black"
        private const val INK_BLACK = "ink-black"
        private const val WARM_WHITE = "warm-white"
        private val THEMES = setOf(WARM_BLACK, INK_BLACK, WARM_WHITE)

        private fun splash(theme: String): Int = when (theme) {
            WARM_BLACK -> R.style.Theme_World_WarmBlack
            INK_BLACK -> R.style.Theme_World_InkBlack
            WARM_WHITE -> R.style.Theme_World_WarmWhite
            else -> throw IllegalArgumentException("Unknown theme: $theme")
        }
    }
}
