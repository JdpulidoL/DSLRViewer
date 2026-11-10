plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
}

// CI passes -PversionName=1.2.3 -PversionCode=10203 (derived from the git tag).
val appVersionName = (findProperty("versionName") as String?) ?: "0.1.0"
val appVersionCode = ((findProperty("versionCode") as String?) ?: "1").toInt()

android {
    namespace = "app.lumen.tether"
    compileSdk = 35

    defaultConfig {
        applicationId = "app.lumen.tether"
        minSdk = 26
        targetSdk = 35
        versionCode = appVersionCode
        versionName = appVersionName
        ndk { abiFilters += listOf("arm64-v8a", "armeabi-v7a", "x86_64") }
    }

    signingConfigs {
        // Only configured when the release secrets are present (see .github/workflows/release.yml).
        val keystore = System.getenv("ANDROID_KEYSTORE_PATH")
        if (keystore != null) {
            create("release") {
                storeFile = file(keystore)
                storePassword = System.getenv("ANDROID_KEYSTORE_PASSWORD")
                keyAlias = System.getenv("ANDROID_KEY_ALIAS")
                keyPassword = System.getenv("ANDROID_KEY_PASSWORD")
            }
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
            signingConfigs.findByName("release")?.let { signingConfig = it }
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions { jvmTarget = "17" }
    buildFeatures { compose = true }

    packaging { jniLibs.keepDebugSymbols += "**/*.so" }
}

dependencies {
    val composeBom = platform("androidx.compose:compose-bom:2024.10.01")
    implementation(composeBom)
    implementation("androidx.activity:activity-compose:1.9.3")
    implementation("androidx.lifecycle:lifecycle-viewmodel-compose:2.8.7")
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.material3:material3-window-size-class")
    implementation("androidx.core:core-ktx:1.15.0")
    // UniFFI's generated Kotlin talks to the Rust library through JNA.
    implementation("net.java.dev.jna:jna:5.15.0@aar")
    testImplementation("junit:junit:4.13.2")
}

// ---- Rust -> Android glue -------------------------------------------------------------------------
// Needs: rustup targets for the 3 ABIs, `cargo install cargo-ndk`, ANDROID_NDK_HOME.
val repoRoot = rootProject.projectDir.parentFile
val jniOut = layout.projectDirectory.dir("src/main/jniLibs").asFile
val uniffiOut = layout.buildDirectory.dir("generated/uniffi/kotlin").get().asFile

val cargoNdk = tasks.register<Exec>("cargoNdk") {
    group = "rust"
    workingDir = repoRoot
    // Always optimised: the engine decodes JPEGs and runs peaking per frame, a debug build is unusably slow.
    commandLine(
        "cargo", "ndk",
        "-t", "arm64-v8a", "-t", "armeabi-v7a", "-t", "x86_64",
        "-o", jniOut.path,
        "build", "--release", "-p", "lumen-android", "--locked",
    )
}

val uniffiBindgen = tasks.register<Exec>("uniffiBindgen") {
    group = "rust"
    dependsOn(cargoNdk)
    workingDir = repoRoot
    // Library mode: the bindings are read from the metadata embedded in the freshly built .so.
    commandLine(
        "cargo", "run", "--locked", "-p", "lumen-android", "--features", "bindgen", "--bin", "uniffi-bindgen", "--",
        "generate", "--library", "target/aarch64-linux-android/release/liblumen_android.so",
        "--language", "kotlin", "--out-dir", uniffiOut.path,
    )
}

android.sourceSets["main"].java.srcDir(uniffiOut)
tasks.matching { it.name == "preBuild" }.configureEach { dependsOn(uniffiBindgen) }
