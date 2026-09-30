# Android

The app lives in `apps/android`: Kotlin, Jetpack Compose, Gradle 9.8. Gradle builds the Rust core
through `cargo xtask mobile android`, so no `cargo-ndk` is needed.

## Toolchain

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

Keep the `export` lines in your shell profile. `cargo xtask mobile android` adds the Rust targets
itself. The Gradle daemon picks JDK 21 on its own, whatever `java` your shell runs.

## Build and test

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

`./gradlew assembleRelease bundleRelease` builds unsigned release outputs unless the
`SDRMM_KEYSTORE*` variables are set.

On the emulator, the server on your computer is `10.0.2.2`. Move the phone with
`adb emu geo fix 13.4050 52.5200`.

## Android Auto

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

## CI

The `android` job runs the Gradle checks, unit tests, `assembleDebug` and the alignment check.
The nightly `android-device` job runs `connectedDebugAndroidTest` on an API 36 emulator.
