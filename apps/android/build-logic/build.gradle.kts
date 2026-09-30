plugins {
    `kotlin-dsl`
}

dependencies {
    compileOnly(libs.android.gradle.api)
    testImplementation(libs.junit4)
    testImplementation(libs.truth)
}

gradlePlugin {
    plugins {
        create("rustCore") {
            id = "sdrmm.rust-core"
            implementationClass = "dev.newspicel.sdrmm.build.RustCorePlugin"
        }
    }
}
