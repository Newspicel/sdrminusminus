# Phone apps

Both apps sit on `crates/mobile-core`, a Rust core bound to Swift and Kotlin with UniFFI. It
handles pairing, the pinned TLS client, missions, and pose fusion.

| Command | Runs |
|---|---|
| `cargo xtask mobile bindings` | Swift and Kotlin bindings |
| `cargo xtask mobile check` | Clippy for the iOS and Android targets; `--ios` or `--android` picks one |
| `cargo xtask mobile ios` | The core as an XCFramework |
| `cargo xtask mobile android` | The core for `arm64-v8a` and `x86_64`; `--abi` limits it |

`cargo xtask check` fails if `mobile-core` pulls the engine, server, DSP, or native libraries into a
phone build.

## iPhone

The app lives in `apps/ios` and needs Xcode 27.

| Command | Runs |
|---|---|
| `cargo xtask ios generate` | The core and the Xcode project, through XcodeGen |
| `cargo xtask ios build` | The app for the simulator |
| `cargo xtask ios test` | Unit tests; `--ui` adds UI tests, `--floor` an iOS 18.1 run, `--only <test>` a subset |
| `cargo xtask ios lint` | swift-format and the comment check |
| `cargo xtask ios e2e` | The app against a real `sdrmm` |
| `cargo xtask ios archive` | A Release archive in `target/ios/`, after a privacy manifest check |

Tests run on an iPhone 17 simulator with iOS 27; `SDRMM_IOS_SIMULATOR` names another one.

To run it on your own iPhone, run `cargo xtask ios generate`, open `apps/ios/SDRmm.xcodeproj`, and
set your team in `apps/ios/Config/Local.xcconfig` as `DEVELOPMENT_TEAM = <team>`.

## Android

The app lives in `apps/android`: Kotlin, Jetpack Compose, Gradle 9.8, API 29 and up. Gradle
builds the Rust core through `cargo xtask mobile android`, so no `cargo-ndk` is needed.

### Toolchain

Install JDK 21, the Android command line tools, and then:

```sh
export ANDROID_HOME="$HOME/Library/Android/sdk"
export PATH="$ANDROID_HOME/cmdline-tools/latest/bin:$ANDROID_HOME/platform-tools:$ANDROID_HOME/emulator:$PATH"
yes | sdkmanager --licenses
sdkmanager "platform-tools" "emulator" \
  "platforms;android-37.0" "build-tools;37.0.0" "ndk;30.0.16248370" \
  "system-images;android-36;google_apis_playstore;arm64-v8a" "extras;google;auto"
export ANDROID_NDK_HOME="$ANDROID_HOME/ndk/30.0.16248370"
avdmanager create avd -n sdrmm-api36 -k "system-images;android-36;google_apis_playstore;arm64-v8a" -d pixel_9
```

Keep the `export` lines in your shell profile. `cargo xtask mobile android` adds the Rust targets.
The Gradle daemon picks JDK 21 whatever `java` your shell runs. Add `-Psdrmm.abis=arm64-v8a` to a
Gradle command to build the core for one ABI only.

### Build and test

Run these in `apps/android`:

```sh
./gradlew spotlessApply
./gradlew spotlessCheck checkSourceRules lintDebug testDebugUnitTest assembleDebug
./gradlew :app:testDebugUnitTest --tests "dev.newspicel.sdrmm.sensors.*"
./gradlew -p build-logic test
```

The debug APK lands in `app/build/outputs/apk/debug/app-debug.apk`. Check its 16 KB page
alignment:

```sh
"$ANDROID_HOME/build-tools/37.0.0/zipalign" -c -P 16 -v 4 app/build/outputs/apk/debug/app-debug.apk
```

Device tests run on the emulator:

```sh
emulator -avd sdrmm-api36 -no-snapshot -no-audio &
adb wait-for-device
./gradlew connectedDebugAndroidTest
adb install -r app/build/outputs/apk/debug/app-debug.apk
```

`./gradlew assembleRelease bundleRelease` signs release outputs when `SDRMM_KEYSTORE`,
`SDRMM_KEYSTORE_PASSWORD`, `SDRMM_KEY_ALIAS`, and `SDRMM_KEY_PASSWORD` are set, and leaves them
unsigned otherwise.

On the emulator, the server on your computer is `10.0.2.2`. Move the phone with
`adb emu geo fix 13.4050 52.5200`.

### Android Auto

1. Use a phone with Android Auto, or the Play Store emulator signed in with Android Auto
   installed.
2. In Android Auto, tap **Version** ten times, open **Developer settings**, turn on
   **Unknown sources**, then **Start head unit server**.
3. Run the head unit:

   ```sh
   apps/android/gradlew -p apps/android installDebug
   adb forward tcp:5277 tcp:5277
   "$ANDROID_HOME/extras/google/auto/desktop-head-unit"
   ```

4. Open SDR-- from the head unit's launcher. Type `day` or `night` in the head unit console to
   switch the map style.

Check that **Missions** lists over the map, the DF panel updates about once a second,
**Navigate** opens the car's navigation app, and a moved target posts a `New target` alert.

### CI

The `mobile` job runs `mobile check --android` and `mobile bindings`. The `ios` job runs
`mobile check --ios`, `ios lint`, and `ios test`. The `android` job runs the `build-logic` tests,
the Gradle checks, unit tests, `assembleDebug`, and the alignment check.

Nightly, `ios-ui` runs `ios test --ui --floor`, `ios-e2e` runs `ios e2e`, and `android-device` runs
`connectedDebugAndroidTest` on an API 36 emulator.
