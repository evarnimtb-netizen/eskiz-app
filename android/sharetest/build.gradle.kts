plugins {
    id("com.android.application")
}

// A tiny debug-only test app for android/files_smoke.sh (A4): it receives what Eskiz shares and
// reads or writes the clipboard, and logs the MIME type, size and SHA-256 of what it saw. It is
// never shipped, and no release task builds it (`tools/ci_android.sh` names the :app: tasks).
android {
    namespace = "app.eskiz.sharetest"
    compileSdk = 36

    defaultConfig {
        applicationId = "app.eskiz.sharetest"
        minSdk = 29
        targetSdk = 36
        versionCode = 1
        versionName = "1.0"
    }
    signingConfigs {
        // The same public debug key as :app (see ../DEBUG_KEYSTORE.md).
        getByName("debug") {
            storeFile = rootProject.file("easel-debug.keystore")
            storePassword = "easel-debug"
            keyAlias = "easeldebug"
            keyPassword = "easel-debug"
        }
    }
    buildTypes {
        release {
            isMinifyEnabled = false
            signingConfig = signingConfigs.getByName("debug")
        }
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    lint { checkReleaseBuilds = false }
}
