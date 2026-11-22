plugins {
    id("com.android.application")
}

// 仓库根（android/ 的上一级）：cargo-ndk 在此执行，产物直接落到 app/src/main/jniLibs
val repoRoot: File = rootProject.projectDir.parentFile
val jniLibsDir: File = layout.projectDirectory.dir("src/main/jniLibs").asFile

android {
    namespace = "com.music.zplayer"
    compileSdk = 35
    ndkVersion = "27.0.12077973"

    defaultConfig {
        applicationId = "com.music.zplayer"
        // Slint Android 后端最低要求 minSdk 26
        minSdk = 26
        targetSdk = 35
        versionCode = 7
        versionName = "0.2.7"

        ndk {
            abiFilters += "arm64-v8a"
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    sourceSets["main"].jniLibs.srcDir("src/main/jniLibs")
}

// 调用 cargo-ndk 交叉编译 cdylib（libzplayer_music.so），输出到 jniLibs
val cargoNdkBuild by tasks.registering(Exec::class) {
    group = "rust"
    description = "cargo ndk 构建 arm64-v8a 的 libzplayer_music.so"
    workingDir = repoRoot
    commandLine(
        "cargo", "ndk",
        "--target=arm64-v8a",
        "--platform", "26",
        "-o", jniLibsDir.absolutePath,
        "build", "--release", "--lib", "--no-default-features", "--features", "mobile",
    )
    environment("ANDROID_NDK_HOME", android.ndkDirectory.absolutePath)
    environment("ANDROID_HOME", android.sdkDirectory.absolutePath)
    // Slint 的 i-slint-backend-android-activity build.rs（依赖 android-build crate）在
    // 未指定平台时会取 SDK platforms/ 下“最高版本”目录并要求其含 android.jar；CI runner
    // 预装了 android-37.2-beta3 等预览目录但无 android.jar，导致 panic
    // “No Android platforms found”。这里显式锁定与 compileSdk 一致的 android-35 与
    // build-tools 版本，避免依赖目录扫描结果。
    environment("ANDROID_PLATFORM", "android-35")
    environment("ANDROID_BUILD_TOOLS_VERSION", "35.0.0")
    // 增量判定：Rust 源码、UI、构建脚本或 Cargo.toml 变更才重跑
    inputs.dir(repoRoot.resolve("src"))
    inputs.dir(repoRoot.resolve("ui"))
    inputs.file(repoRoot.resolve("build.rs"))
    inputs.file(repoRoot.resolve("Cargo.toml"))
    inputs.file(repoRoot.resolve("Cargo.lock"))
    outputs.dir(jniLibsDir)
}

tasks.named("preBuild") {
    dependsOn(cargoNdkBuild)
}
