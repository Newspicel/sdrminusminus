plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.compose)
    alias(libs.plugins.spotless)
    id("sdrmm.rust-core")
}

android {
    namespace = "dev.newspicel.sdrmm"
    compileSdk = 37
    ndkVersion = "30.0.16248370"
    buildToolsVersion = "37.0.0"

    defaultConfig {
        applicationId = "dev.newspicel.sdrmm"
        minSdk = 29
        targetSdk = 37
        versionCode = providers.gradleProperty("sdrmmVersionCode").get().toInt()
        versionName = providers.gradleProperty("sdrmmVersionName").get()
        testInstrumentationRunner = "dev.newspicel.sdrmm.SdrmmTestRunner"
        ndk {
            abiFilters += setOf("arm64-v8a", "x86_64")
        }
    }

    signingConfigs {
        create("release") {
            storeFile = providers.environmentVariable("SDRMM_KEYSTORE").map { file(it) }.orNull
            storePassword = providers.environmentVariable("SDRMM_KEYSTORE_PASSWORD").orNull
            keyAlias = providers.environmentVariable("SDRMM_KEY_ALIAS").orNull
            keyPassword = providers.environmentVariable("SDRMM_KEY_PASSWORD").orNull
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
            signingConfig = signingConfigs.getByName("release").takeIf { it.storeFile != null }
        }
    }

    buildFeatures {
        compose = true
        buildConfig = true
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_21
        targetCompatibility = JavaVersion.VERSION_21
    }

    sourceSets {
        getByName("test").kotlin.directories += "src/sharedTest/java"
        getByName("androidTest").kotlin.directories += "src/sharedTest/java"
    }

    testOptions {
        unitTests.isIncludeAndroidResources = true
        unitTests.all { it.jvmArgs("--add-opens=java.base/jdk.internal.access=ALL-UNNAMED", "--add-opens=java.base/java.io=ALL-UNNAMED") }
        animationsDisabled = true
    }

    lint {
        warningsAsErrors = true
        abortOnError = true
        disable += setOf("NewerVersionAvailable", "GradleDependency", "AndroidGradlePluginVersion", "LogNotTimber")
    }
}

kotlin {
    jvmToolchain(21)
}

rustCore {
    workspaceDir.set(rootProject.layout.projectDirectory.dir("../.."))
    cratePackage.set("sdrmm-mobile-core")
    libraryName.set("sdrmm_mobile_core")
    abis.set(providers.gradleProperty("sdrmm.abis").map { it.split(",") })
}

spotless {
    kotlin {
        target("src/**/*.kt")
        ktlint(libs.versions.ktlint.get())
            .editorConfigOverride(mapOf("ktlint_function_naming_ignore_when_annotated_with" to "Composable"))
    }
    kotlinGradle {
        target("*.gradle.kts")
        ktlint(libs.versions.ktlint.get())
    }
}

dependencies {
    implementation(platform(libs.compose.bom))
    implementation(libs.compose.ui)
    implementation(libs.compose.ui.graphics)
    implementation(libs.compose.ui.tooling.preview)
    implementation(libs.compose.material3)
    implementation(libs.androidx.core.ktx)
    implementation(libs.androidx.activity.compose)
    implementation(libs.androidx.lifecycle.runtime.compose)
    implementation(libs.androidx.lifecycle.viewmodel.compose)
    implementation(libs.androidx.lifecycle.service)
    implementation(libs.androidx.lifecycle.process)
    implementation(libs.androidx.lifecycle.viewmodel.navigation3)
    implementation(libs.androidx.navigation3.runtime)
    implementation(libs.androidx.navigation3.ui)
    implementation(libs.datastore.preferences)
    implementation(libs.camera.core)
    implementation(libs.camera.camera2)
    implementation(libs.camera.lifecycle)
    implementation(libs.camera.compose)
    implementation(libs.zxing.core)
    implementation(libs.maplibre.android)
    implementation(libs.car.app)
    implementation(libs.car.app.projected)
    implementation(variantOf(libs.jna) { artifactType("aar") })
    implementation(libs.kotlinx.coroutines.android)
    debugImplementation(libs.compose.ui.tooling)
    debugImplementation(libs.compose.ui.test.manifest)
    testImplementation(libs.junit4)
    testImplementation(libs.truth)
    testImplementation(libs.turbine)
    testImplementation(libs.kotlinx.coroutines.test)
    testImplementation(libs.robolectric)
    testImplementation(libs.androidx.test.core)
    testImplementation(libs.androidx.test.ext.junit)
    testImplementation(platform(libs.compose.bom))
    testImplementation(libs.compose.ui.test.junit4)
    testImplementation(libs.car.app.testing)
    testImplementation(libs.espresso.core)
    androidTestImplementation(platform(libs.compose.bom))
    androidTestImplementation(libs.compose.ui.test.junit4)
    androidTestImplementation(libs.androidx.test.runner)
    androidTestImplementation(libs.androidx.test.rules)
    androidTestImplementation(libs.androidx.test.ext.junit)
    androidTestImplementation(libs.espresso.core)
    androidTestImplementation(libs.espresso.intents)
    androidTestImplementation(libs.truth)
    androidTestImplementation(libs.turbine)
    androidTestImplementation(libs.kotlinx.coroutines.test)
}
