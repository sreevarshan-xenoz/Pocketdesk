pluginManagement {
    repositories {
        google()
        mavenCentral()
        gradlePluginPortal()
    }
}

dependencyResolutionManagement {
    repositoriesMode.set(RepositoriesMode.FAIL_ON_PROJECT_REPOS)
    repositories {
        google()
        mavenCentral()
    }
}

rootProject.name = "pocketdesk-android"
include(":app")
// N1: the device checks' fixture app (never in Pocketdesk's APK).
include(":fixture")
// Item 9: the wire the phone app and the watch app share, and the Wear OS app
// itself (a tile and a complication; never part of the phone's APK).
include(":watchlink")
include(":wear")
