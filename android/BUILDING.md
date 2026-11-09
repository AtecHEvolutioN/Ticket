# Building the Android test app

The app is plain Java with Android framework APIs and no third-party runtime
dependencies. It supports Android 8.0 (API 26) or later and targets Android 15
(API 35). There are no bundled API credentials or native ABI restrictions.

## Android Studio / Gradle

Open this directory in Android Studio with a JDK 17 or later, Android SDK platform
35, and build tools 35.0.0 installed. The wrapper pins Gradle 8.9 and verifies the
official distribution checksum. The Android Gradle plugin is pinned to 8.7.3.

```bash
./gradlew assembleDebug
```

The Gradle output is `app/build/outputs/apk/debug/app-debug.apk`. The debug build
uses test signing but keeps `android:debuggable` disabled. Production distribution
should use a separate signing configuration and a stable private signing key.

## SDK-only build used for this test APK

The checked-in script compiles resources with AAPT2, Java with the JDK compiler,
DEX with D8, aligns the archive, and signs/verifies it with apksigner. It needs
Java 17 or later (including compiler/jartool modules), Python 3, and these official
SDK packages:

```bash
sdkmanager 'platforms;android-35' 'build-tools;35.0.0' 'platform-tools'
ANDROID_SDK_ROOT=/path/to/android-sdk ./build-apk.sh
```

Output: `../dist/ticket-watch-android.apk`, with a SHA-256 checksum alongside it.
The script creates an ignored local test signing key at `.signing/debug.keystore`;
keep that file to sign compatible updates. It never includes the signing key or
credentials in the APK. `TICKET_ANDROID_DEBUG_KEYSTORE` can select an existing
local test keystore instead.

The built APK is a test artifact, not an app-store publication. Install it through
Android's normal package installer. On first installation, Android may ask you to
allow app installation from the browser or file manager you use.

## Continuous integration

The scoped Android workflow builds and verifies the app and internal test APKs
when Android sources change, or on manual dispatch. Its downloadable artifact
contains only the main APK and SHA-256 checksum, never the instrumentation APK
or signing key. The workflow compiles instrumentation but does not run an emulator.

CI creates an ephemeral test certificate for each fresh job. Such APKs may require
uninstalling a build signed with a different certificate before installation.
Preserve the local `.signing/debug.keystore` for compatible local updates. A stable
production signing key would need a separately configured private CI secret.
