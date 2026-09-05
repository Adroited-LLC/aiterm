import java.util.Properties

plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.compose)
    alias(libs.plugins.kotlin.serialization)
}

val rustJniOutput = layout.buildDirectory.get().dir("generated/rustJniLibs/main").asFile
val localProperties = Properties().apply {
    val file = rootProject.layout.projectDirectory.file("local.properties").asFile
    if (file.isFile) file.inputStream().use(::load)
}
val androidSdk = System.getenv("ANDROID_HOME")
    ?: System.getenv("ANDROID_SDK_ROOT")
    ?: localProperties.getProperty("sdk.dir")
    ?: error("Android SDK path is required in ANDROID_HOME, ANDROID_SDK_ROOT, or local.properties")

val buildRustQuic = tasks.register<Exec>("buildRustQuic") {
    val bridge = rootProject.layout.projectDirectory.dir("quic-bridge")
    workingDir(bridge)
    environment(
        "ANDROID_NDK_HOME",
        System.getenv("ANDROID_NDK_HOME")
            ?: "$androidSdk/ndk/27.0.12077973",
    )
    commandLine(
        "cargo",
        "ndk",
        "-t",
        "arm64-v8a",
        "-o",
        rustJniOutput.absolutePath,
        "build",
        "--release",
    )
    inputs.file(bridge.file("Cargo.toml"))
    inputs.file(bridge.file("Cargo.lock"))
    inputs.dir(bridge.dir("src"))
    inputs.file(rootProject.layout.projectDirectory.file("../relay-protocol/Cargo.toml"))
    inputs.dir(rootProject.layout.projectDirectory.dir("../relay-protocol/src"))
    outputs.dir(rustJniOutput)
}

android {
    namespace = "com.adroited.aiterm"
    compileSdk = 37

    defaultConfig {
        applicationId = "com.adroited.aiterm"
        minSdk = 26
        targetSdk = 37
        versionCode = 8
        versionName = "0.3.5"

        // Our native QUIC bridge is ARM64-only. Declaring the supported ABI
        // also prevents dependency AARs from advertising unusable variants.
        ndk {
            abiFilters += "arm64-v8a"
        }

        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
    }

    ndkVersion = "27.0.12077973"

    sourceSets.named("main") {
        jniLibs.srcDir(rustJniOutput)
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

    buildFeatures {
        compose = true
        // The identity test reads BuildConfig.APPLICATION_ID, which AGP only
        // generates when this is on.
        buildConfig = true
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    packaging {
        resources {
            excludes += "/META-INF/{AL2.0,LGPL2.1}"
        }
    }
}

tasks.named("preBuild").configure {
    dependsOn(buildRustQuic)
}

java {
    toolchain {
        languageVersion.set(JavaLanguageVersion.of(21))
    }
}

dependencies {
    implementation(libs.androidx.core.ktx)
    implementation(libs.androidx.exifinterface)
    implementation(libs.androidx.activity.compose)
    implementation(libs.androidx.lifecycle.runtime.ktx)
    implementation(libs.androidx.lifecycle.runtime.compose)
    implementation(libs.androidx.lifecycle.process)
    implementation(libs.androidx.lifecycle.viewmodel.ktx)
    implementation(libs.androidx.lifecycle.viewmodel.compose)
    implementation(libs.androidx.navigation.compose)

    implementation(platform(libs.compose.bom))
    implementation(libs.compose.ui)
    implementation(libs.compose.ui.graphics)
    implementation(libs.compose.ui.tooling.preview)
    implementation(libs.compose.material3)
    implementation(libs.compose.material.icons.extended)
    implementation(libs.coil.compose)
    implementation(libs.coil.svg)

    // Task 8: QR enrollment scans the aiterm://pair payload with CameraX + ML Kit.
    implementation(libs.androidx.camera.core)
    implementation(libs.androidx.camera.camera2)
    implementation(libs.androidx.camera.lifecycle)
    implementation(libs.androidx.camera.view)
    implementation(libs.mlkit.barcode.scanning)

    // Task 8: biometric / device-credential gate in front of the Keystore key.
    implementation(libs.androidx.biometric)

    // Tasks 8 and 9: pinned TLS enrollment call and the /v1/ws transport.
    implementation(libs.conscrypt.android)
    implementation(libs.okhttp)
    implementation(libs.okhttp.tls)

    // Task 9: the wire format is binary CBOR envelopes.
    implementation(libs.kotlinx.serialization.cbor)
    implementation(libs.kotlinx.serialization.json)
    implementation(libs.kotlinx.coroutines.android)

    // Optional network stack selected at pairing time. It carries the same
    // pinned TLS/WebSocket protocol as the native AITerm stack.
    implementation("computer.iroh:iroh:1.1.0") {
        exclude(group = "net.java.dev.jna", module = "jna")
    }
    // Maven's iroh-android 1.1.0 predates upstream's Android 16 KB page-size
    // fix. This ARM64 AAR is rebuilt reproducibly from the same v1.1.0 tag;
    // replace it with the first compatible upstream release when published.
    implementation(files("libs/iroh-android-1.1.0-page16-arm64.aar"))
    implementation("net.java.dev.jna:jna:5.15.0@aar")

    debugImplementation(libs.compose.ui.tooling)
    debugImplementation(libs.compose.ui.test.manifest)

    testImplementation(libs.junit)
    testImplementation(libs.kotlinx.coroutines.test)
    testImplementation(libs.okhttp.mockwebserver)

    androidTestImplementation(platform(libs.compose.bom))
    androidTestImplementation(libs.androidx.test.junit)
    androidTestImplementation(libs.androidx.test.espresso.core)
    androidTestImplementation(libs.compose.ui.test.junit4)
}
