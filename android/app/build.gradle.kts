plugins {
    id("com.android.application")
}

// 仓库根（android/ 的上一级）：cargo-ndk 在此执行，产物直接落到 app/src/main/jniLibs
val repoRoot: File = rootProject.projectDir.parentFile
val jniLibsDir: File = layout.projectDirectory.dir("src/main/jniLibs").asFile
// MuMu 等 x86_64 模拟器会把含 arm64 的 APK 判成 arm64 primaryAbi，加载任何 .so 都走
// nativebridge(houdini) 翻译导致 UnsatisfiedLinkError。-Px86Only 只打 x86_64 单 ABI，
// 强制 PM 判成 x86_64，绕过翻译层；正常构建仍打 arm64-v8a + x86_64 双 ABI。
val x86Only: Boolean = project.hasProperty("x86Only")

android {
    // 与 applicationId 保持一致：R / BuildConfig 及 Java 源码包名均为 com.zplayer.music
    namespace = "com.zplayer.music"
    compileSdk = 35
    ndkVersion = "27.0.12077973"

    defaultConfig {
        // 保持与既有 cargo-apk 安装包一致的包名，覆盖安装不丢数据
        applicationId = "com.zplayer.music"
        // Slint Android 后端最低要求 minSdk 26
        minSdk = 26
        targetSdk = 35
        versionCode = 7
        versionName = "0.2.7"

        ndk {
            // arm64-v8a：真机；x86_64：MuMu / Android Studio 模拟器（其 ARM 翻译层
            // 链接本 .so 失败，须原生 x86_64 才能跑）。两个 ABI 都打进 APK。
            if (!x86Only) {
                abiFilters += "arm64-v8a"
            }
            abiFilters += "x86_64"
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

dependencies {
    // 系统启动屏保持：setKeepOnScreenCondition 让原生启动画面停留到 Slint 首帧，
    // 消除“原生 splash → 黑 surface → Slint 启动动画”的断层（Android 12+ 走平台实现）。
    implementation("androidx.core:core-splashscreen:1.0.1")
}

// 调用 cargo-ndk 交叉编译 cdylib（libzplayer_music.so），输出到 jniLibs
val cargoNdkBuild by tasks.registering(Exec::class) {
    group = "rust"
    description = "cargo ndk 构建 arm64-v8a + x86_64 的 libzplayer_music.so"
    workingDir = repoRoot
    val ndkTargets = if (x86Only) {
        listOf("--target=x86_64")
    } else {
        listOf("--target=arm64-v8a", "--target=x86_64")
    }
    commandLine(
        listOf("cargo", "ndk") + ndkTargets + listOf(
            "--platform", "26",
            "-o", jniLibsDir.absolutePath,
            "build", "--release", "--lib", "--no-default-features", "--features", "mobile",
        )
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
    // 关键：cargo ndk 的 --platform 26 会让 build.rs 把 ANDROID_PLATFORM 解析为
    // android-26，但本机只装了 29/30/34/35，探测不到 android-26 仍会 panic。
    // 直接给出 android.jar / d8.jar 绝对路径（优先级最高），彻底绕过平台目录扫描。
    val sdkDir = android.sdkDirectory
    environment("ANDROID_JAR", File(sdkDir, "platforms/android-35/android.jar").absolutePath)
    environment("ANDROID_D8_JAR", File(sdkDir, "build-tools/35.0.0/lib/d8.jar").absolutePath)
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
