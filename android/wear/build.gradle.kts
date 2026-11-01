import org.jetbrains.kotlin.gradle.dsl.JvmTarget

// Item 9: Pocketdesk on a Wear OS watch: a tile, a complication and a short
// list. Its own app (installed on the watch, never part of the phone's APK),
// talking to the phone app over Bluetooth with the watchlink module. It uses
// no Google Play services, no Data Layer and no Firebase: only AndroidX
// libraries (Apache-2.0), so it can go to F-Droid beside the phone app.
plugins {
    id("com.android.application")
}

android {
    namespace = "dev.pocketdesk.wear"
    compileSdk = 37

    defaultConfig {
        applicationId = "dev.pocketdesk.wear"
        // Wear OS 3 and later: the tiles and complications libraries start there.
        minSdk = 30
        targetSdk = 36
        versionCode = 1
        versionName = "0.1.0"
    }

    // F-Droid and IzzyOnDroid refuse the Google-encrypted dependency block.
    dependenciesInfo {
        includeInApk = false
        includeInBundle = false
    }

    buildTypes {
        release {
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(
                getDefaultProguardFile("proguard-android-optimize.txt"),
                "proguard-rules.pro",
            )
        }
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
    implementation(project(":watchlink"))
    // The tile: layouts and the service the watch binds (Apache-2.0).
    implementation("androidx.wear.tiles:tiles:1.6.2")
    implementation("androidx.wear.protolayout:protolayout:1.4.2")
    // The complication: the data source the watch face asks (Apache-2.0).
    implementation("androidx.wear.watchface:watchface-complications-data-source:1.3.0")
    implementation("androidx.core:core-ktx:1.16.0")
    // A tile's request answers with a future.
    implementation("androidx.concurrent:concurrent-futures:1.2.0")
    testImplementation("junit:junit:4.13.2")
}
