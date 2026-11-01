import org.jetbrains.kotlin.gradle.dsl.JvmTarget

// The wire between the phone app and the watch app (item 9 of next-plan-3):
// frames, keys, the rules a phone applies to what a watch asks, and the pure
// code both sides test. Plain Kotlin, no Android classes and no dependency of
// any kind: it is in the phone's APK and the watch's, and it must stay
// F-Droid clean.
plugins {
    id("com.android.library")
}

android {
    namespace = "dev.pocketdesk.watchlink"
    compileSdk = 37

    defaultConfig {
        minSdk = 26
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

kotlin {
    compilerOptions {
        jvmTarget.set(JvmTarget.JVM_17)
    }
}

dependencies {
    testImplementation("junit:junit:4.13.2")
}
