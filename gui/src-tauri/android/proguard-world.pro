# JNI calls these methods by name, including when no Tauri Activity exists.
-keep class dev.fastctx.world.WorldRuntime { *; }
# The Tauri plugin manager discovers commands and callbacks by reflection.
-keep class dev.fastctx.world.WorldPlugin { *; }
# The page calls WorldFrame's bridge by its method names.
-keepclassmembers class dev.fastctx.world.WorldFrame$Bridge { @android.webkit.JavascriptInterface <methods>; }
