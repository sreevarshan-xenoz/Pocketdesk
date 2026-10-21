// N1: the fixture the device checks drive (scripts/device_checks.py). Its own
// app, never part of Pocketdesk's APK, with no dependencies.
plugins {
    id("com.android.application")
}

android {
    namespace = "dev.pocketdesk.fixture"
    compileSdk = 37

    defaultConfig {
        applicationId = "dev.pocketdesk.fixture"
        // Notification.MessagingStyle with a Person (API 28); a test phone has more.
        minSdk = 28
        targetSdk = 37
        versionCode = 1
        versionName = "1"
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

kotlin {
    compilerOptions {
        jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17)
    }
}
