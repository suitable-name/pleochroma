plugins {
    id("com.android.application")
}

// The ABIs to build. Phones and tablets are arm64-v8a. For the emulator add x86_64 (and
// `rustup target add x86_64-linux-android`):  gradlew assembleDebug -PrustAbis=arm64-v8a,x86_64
val rustAbis: List<String> =
    (providers.gradleProperty("rustAbis").orNull ?: "arm64-v8a")
        .split(",")
        .map { it.trim() }
        .filter { it.isNotEmpty() }

android {
    namespace = "com.indicatrixsuite.mobile"
    // Compile against the installed SDK platform (android-37.0).
    compileSdk = 37

    defaultConfig {
        applicationId = "com.indicatrixsuite.mobile"
        minSdk = 28
        targetSdk = 37
        versionCode = 12
        versionName = "0.12.0"

        ndk {
            abiFilters += rustAbis
        }
    }

    buildTypes {
        release {
            // R8 keeps MainActivity whole (see proguard-rules.pro): Rust calls its methods
            // by name. Signing is the owner's job; this build type is unsigned.
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(
                getDefaultProguardFile("proguard-android-optimize.txt"),
                "proguard-rules.pro",
            )
        }
    }

    // Built-in Kotlin takes its JVM target from targetCompatibility.
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    packaging {
        jniLibs {
            // Store the .so uncompressed and page-aligned (16 KB devices, Play requirement).
            useLegacyPackaging = false
        }
    }
}

dependencies {
    // FileProvider (share sheet) and WindowCompat.
    implementation("androidx.core:core-ktx:1.19.1")
    // The in-app pairing scanner (ScannerActivity): camera permission request, CameraX
    // preview and frame analysis, and the ZXing QR decoder.
    implementation("androidx.activity:activity-ktx:1.10.1")
    implementation("androidx.camera:camera-core:1.4.2")
    implementation("androidx.camera:camera-camera2:1.4.2")
    implementation("androidx.camera:camera-lifecycle:1.4.2")
    implementation("androidx.camera:camera-view:1.4.2")
    implementation("com.google.zxing:core:3.5.3")
}

// ---------------------------------------------------------------------------------------
// Rust. `cargo ndk` builds libpleochroma_mobile_android.so for every ABI in `rustAbis`
// into src/main/jniLibs/<abi>/. The JNI-libs merge tasks wait for it, so the APK never
// carries a stale library (cargo itself stays incremental). The first build compiles
// Skia from source and is slow.
// ---------------------------------------------------------------------------------------

// android/ -> pleochroma-mobile-android/ -> apps/ -> the workspace root (the Cargo workspace).
val cargoWorkspace: File = rootProject.projectDir.resolve("../../..").canonicalFile
val jniLibsDir: File = projectDir.resolve("src/main/jniLibs")

// Slint's Android backend looks for `android.jar` itself and does not find `android-37.0`
// folders, so the compile platform's jar is passed explicitly.
val sdkDirectory: Provider<Directory> = androidComponents.sdkComponents.sdkDirectory
val compileSdkLevel: Int = checkNotNull(android.compileSdk) { "compileSdk is not set" }

fun androidJarIn(sdk: File): File? =
    sdk.resolve("platforms").listFiles()
        ?.filter { it.isDirectory && it.name.startsWith("android-$compileSdkLevel") }
        ?.map { it.resolve("android.jar") }
        ?.firstOrNull { it.isFile }

fun registerCargoTask(taskName: String, release: Boolean): TaskProvider<Exec> =
    tasks.register<Exec>(taskName) {
        group = "rust"
        description =
            "Builds the Rust library with cargo-ndk (${if (release) "android-release" else "android-dev"} profile)."
        workingDir = cargoWorkspace
        // The SDK location is only resolved when the task runs.
        doFirst {
            val sdk = sdkDirectory.get().asFile
            environment("ANDROID_HOME", sdk.absolutePath)
            androidJarIn(sdk)?.let { environment("ANDROID_JAR", it.absolutePath) }
        }
        val command = mutableListOf("cargo", "ndk")
        rustAbis.forEach { abi -> command += listOf("-t", abi) }
        command += listOf("-o", jniLibsDir.absolutePath, "build", "-p", "pleochroma-mobile-android")
        command += listOf("--profile", if (release) "android-release" else "android-dev")
        commandLine(command)
    }

val cargoBuildDebug = registerCargoTask("cargoBuildDebug", release = false)
val cargoBuildRelease = registerCargoTask("cargoBuildRelease", release = true)

tasks.configureEach {
    when (name) {
        "mergeDebugJniLibFolders" -> dependsOn(cargoBuildDebug)
        "mergeReleaseJniLibFolders" -> dependsOn(cargoBuildRelease)
    }
}
