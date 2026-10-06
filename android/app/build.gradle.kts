plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

android {
    namespace = "com.ezrdp.app"
    compileSdk = 35

    defaultConfig {
        applicationId = "com.ezrdp.app"
        minSdk = 26
        targetSdk = 35
        // Goes up with every build (minutes since Sept 2025), so Android offers "Update"
        // instead of treating a new APK as the same app that's already installed.
        val build = ((System.currentTimeMillis() / 60_000L) - 29_270_000L).toInt()
        versionCode = build
        versionName = "1.1.$build"
    }

    signingConfigs {
        // One fixed key kept with the project, so every build can update the last one.
        create("ezrdp") {
            storeFile = file("ezrdp.keystore")
            storePassword = "android"
            keyAlias = "androiddebugkey"
            keyPassword = "android"
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
            signingConfig = signingConfigs.getByName("ezrdp")
        }
        debug {
            signingConfig = signingConfigs.getByName("ezrdp")
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions {
        jvmTarget = "17"
    }
    testOptions {
        unitTests {
            isIncludeAndroidResources = true
            all {
                it.systemProperty("robolectric.dependency.repo.url", "https://maven-central.storage-download.googleapis.com/maven2")
                it.systemProperty("shots.dir", rootDir.resolve("../screenshots").absolutePath)
            }
        }
    }
}

dependencies {
    testImplementation("junit:junit:4.13.2")
    testImplementation("org.robolectric:robolectric:4.14.1")
    testImplementation("androidx.test:core:1.6.1")
}
