import org.jetbrains.kotlin.gradle.dsl.JvmTarget

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.plugin.compose")
}

android {
    namespace = "dev.pocketdesk.android"
    compileSdk = 37

    defaultConfig {
        applicationId = "dev.pocketdesk.android"
        minSdk = 26
        targetSdk = 37
        versionCode = 2
        versionName = "0.2.0"
        // The Rust bridge is built for arm64 only, so no other ABI can run
        // the app; libraries' other ABIs would only add size. A build for an
        // emulator names its ABIs: -Ppocketdesk.abis=arm64-v8a,x86_64.
        ndk {
            abiFilters += (findProperty("pocketdesk.abis") as String? ?: "arm64-v8a").split(",")
        }
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
    }

    // F-Droid and IzzyOnDroid refuse the Google-encrypted dependency block,
    // and it is not needed outside Play.
    dependenciesInfo {
        includeInApk = false
        includeInBundle = false
    }

    lint {
        // x86 ChromeOS devices are left out on purpose: the Rust bridge is
        // built for arm64 only (see abiFilters), so they could not run it.
        disable += "ChromeOsAbiSupport"
    }

    packaging {
        jniLibs {
            // cargo-ndk copies iroh's and iroh-relay's cdylibs (their crates
            // are also cdylib) next to the bridge; nothing loads them.
            excludes += listOf("**/libiroh-*.so", "**/libiroh_relay-*.so")
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = true
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

    buildFeatures {
        compose = true
    }
}

kotlin {
    compilerOptions {
        jvmTarget.set(JvmTarget.JVM_17)
    }
}

dependencies {
    val composeBom = platform("androidx.compose:compose-bom:2026.08.00")
    implementation(composeBom)

    implementation("androidx.activity:activity-compose:1.11.0")
    implementation("androidx.core:core-ktx:1.16.0")
    // N4: turn a copied photo upright (the platform reader has known bugs).
    implementation("androidx.exifinterface:exifinterface:1.4.2")
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.ui:ui-tooling-preview")
    // R4 E6: foldable / large-screen posture (FoldingFeature) for the dual-pane layout.
    implementation("androidx.window:window:1.5.0")
    // Pairing-code scanner: CameraX preview.
    implementation("androidx.camera:camera-camera2:1.6.2")
    implementation("androidx.camera:camera-lifecycle:1.6.2")
    implementation("androidx.camera:camera-view:1.6.2")
    // QR decoding: zxing-cpp (Apache-2.0). Not ML Kit, which is proprietary,
    // pulls in Play services and Firebase components, sends Google usage
    // data, and keeps the app out of F-Droid (T3).
    implementation("io.github.zxing-cpp:android:3.1.1")
    // On-device check that the scanner reads the desktop app's code.
    androidTestImplementation("androidx.test:runner:1.6.2")
    androidTestImplementation("androidx.test.ext:junit:1.2.1")
    debugImplementation("androidx.compose.ui:ui-tooling")
    testImplementation("junit:junit:4.13.2")
}
