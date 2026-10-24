plugins {
    id("com.android.application")
}

// Rust profile for the native library: "debug" (cargo dev profile) or "release".
// Only a build that asks for the debug variant (assembleDebug, installDebug, ...) and nothing
// else uses the debug profile. assembleRelease, bundleRelease and the variant-less build and
// assemble tasks, which also package the release variant, use the release Rust profile.
val taskNames = gradle.startParameter.taskNames
val debugOnly = taskNames.isNotEmpty() &&
    taskNames.none { it.contains("release", ignoreCase = true) } &&
    taskNames.all { it.contains("debug", ignoreCase = true) || it.contains("test", ignoreCase = true) }
val rustProfile = (findProperty("eskiz.rustProfile") as String?) ?: if (debugOnly) "debug" else "release"
// -Peskiz.abis=arm64-v8a limits the build to one ABI (comma separated list).
val abis = ((findProperty("eskiz.abis") as String?) ?: "arm64-v8a,x86_64").split(",").map { it.trim() }
val workspace = rootProject.projectDir.parentFile // eskiz/
val jniLibsDir = layout.projectDirectory.dir("src/main/jniLibs")

android {
    namespace = "app.eskiz"
    compileSdk = 36
    ndkVersion = "30.0.16248370"

    defaultConfig {
        applicationId = "app.eskiz"
        minSdk = 29
        targetSdk = 36
        versionCode = 4
        versionName = "0.1.3"
        ndk { abiFilters += abis }
    }
    // Release signing. The real upload key belongs to the maintainer and is never in this
    // repository. With -Peskiz.keystore=<file> (plus eskiz.keystorePassword, eskiz.keyAlias,
    // eskiz.keyPassword) the release is signed with it; without them it is signed with the
    // debug key, which is enough for sideloading and CI checks but NOT for Play.
    val releaseKeystore = findProperty("eskiz.keystore") as String?
    signingConfigs {
        // The fixed, public, debug-only key (see ../DEBUG_KEYSTORE.md): every debug and debug-signed
        // release APK shares one signature, so a new one installs over an older one. Never for Play.
        getByName("debug") {
            storeFile = rootProject.file("easel-debug.keystore")
            storePassword = "easel-debug"
            keyAlias = "easeldebug"
            keyPassword = "easel-debug"
        }
        if (releaseKeystore != null) {
            create("eskizRelease") {
                storeFile = file(releaseKeystore)
                storePassword = findProperty("eskiz.keystorePassword") as String?
                keyAlias = findProperty("eskiz.keyAlias") as String?
                keyPassword = findProperty("eskiz.keyPassword") as String?
            }
        }
    }
    buildTypes {
        release {
            isMinifyEnabled = false
            signingConfig = signingConfigs.findByName("eskizRelease") ?: signingConfigs.getByName("debug")
        }
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    // The release-time lint-vital pass pulls a large tool closure from Maven and checks nothing
    // Eskiz's Java shim needs; run `./gradlew lint` by hand when wanted.
    lint { checkReleaseBuilds = false }
    packaging {
        jniLibs { useLegacyPackaging = false } // .so stored uncompressed and page aligned
    }
}

// Builds libeskiz_app.so for every ABI into src/main/jniLibs with cargo-ndk.
val cargoNdk = tasks.register<Exec>("cargoNdk") {
    group = "build"
    description = "cargo ndk build of eskiz-app (feature android) into jniLibs"
    workingDir = workspace
    val args = mutableListOf("cargo", "ndk", "--platform", "29")
    abis.forEach { args += listOf("-t", it) }
    args += listOf("-o", jniLibsDir.asFile.absolutePath, "build", "-p", "eskiz-app", "--lib",
        "--no-default-features", "--features", "android")
    if (rustProfile == "release") args += "--release"
    commandLine(args)
    // cargo-ndk finds the NDK through ANDROID_NDK_HOME or ANDROID_HOME/ndk.
    environment("ANDROID_NDK_HOME", android.ndkDirectory.absolutePath)
    inputs.files(fileTree(workspace) {
        include("Cargo.toml", "Cargo.lock", "rust-toolchain.toml", ".cargo/**", "crates/**/Cargo.toml",
            "crates/**/*.rs", "crates/**/build.rs", "crates/**/*.wgsl", "crates/**/assets/**",
            "crates/**/*.md", "crates/**/*.png", "crates/**/*.ttf", "crates/**/*.otf",
            "crates/**/*.json", "crates/**/*.txt", "crates/**/*.glsl", "crates/**/*.bin")
        exclude("**/target/**")
    })
    inputs.property("rustProfile", rustProfile)
    outputs.dir(jniLibsDir)
}
tasks.matching { it.name == "preBuild" }.configureEach { dependsOn(cargoNdk) }

// Licences inside the APK: assets/licenses/ gets LICENSE-MIT, LICENSE-APACHE and the third-party
// notices for the Android build (THIRD_PARTY_NOTICES-android.md, made by tools/gen_notices.sh android).
val licensesDir = layout.projectDirectory.dir("src/main/assets/licenses")
val copyLicenses = tasks.register<Sync>("copyLicenses") {
    group = "build"
    description = "Copies the licence files and third-party notices into assets/licenses"
    from(workspace) { include("LICENSE-MIT", "LICENSE-APACHE") }
    from(workspace) {
        include("THIRD_PARTY_NOTICES-android.md")
        rename("THIRD_PARTY_NOTICES-android.md", "THIRD_PARTY_NOTICES.md")
    }
    into(licensesDir)
}
tasks.matching { it.name == "preBuild" }.configureEach { dependsOn(copyLicenses) }

// bundletool (Google's AAB tool) with its dependencies, for `check_release.sh`. The fat jar is
// only published on GitHub; the library and its closure come from Google's Maven repository.
val bundletool by configurations.creating
dependencies {
    bundletool("com.android.tools.build:bundletool:1.18.3")
}
tasks.register<Sync>("fetchBundletool") {
    group = "build"
    description = "Copies bundletool and its dependencies to app/build/bundletool"
    from(bundletool)
    into(layout.buildDirectory.dir("bundletool"))
}
