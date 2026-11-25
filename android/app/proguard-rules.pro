# Keep Rust core reachable through JNI
-keep class dev.pocketdesk.android.bridge.** { *; }

# Room builds its databases by reflection (`Class.newInstance()` on the
# generated `*_Impl`), and WorkManager's own is one. R8's full mode keeps a
# class's name but not its default constructor, so the release build's
# WorkManager start-up died with NoSuchMethodException on
# `WorkDatabase_Impl.<init>`.
-keep class * extends androidx.room.RoomDatabase { <init>(); }
