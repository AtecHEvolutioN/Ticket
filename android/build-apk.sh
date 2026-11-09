#!/usr/bin/env bash
# Dependency-free debug build using official Android SDK tools and Java17+.
# No credentials, network requests, or machine-specific signing keys are packaged.
set -euo pipefail
project_dir="$(cd "$(dirname "$0")" && pwd)"
sdk_dir="${ANDROID_SDK_ROOT:-${ANDROID_HOME:-/workspace/.android-sdk}}"
tools_dir="$sdk_dir/build-tools/35.0.0"
platform_jar="$sdk_dir/platforms/android-35/android.jar"
build_dir="$project_dir/build/manual"
output_dir="$project_dir/../dist"
keystore_path="${TICKET_ANDROID_DEBUG_KEYSTORE:-$project_dir/.signing/debug.keystore}"
java_bin="${JAVA_HOME:+$JAVA_HOME/bin/}java"
keytool_bin="${JAVA_HOME:+$JAVA_HOME/bin/}keytool"

for path in "$tools_dir/aapt2" "$tools_dir/d8" "$tools_dir/zipalign" "$tools_dir/apksigner" "$platform_jar"; do
    if [[ ! -f "$path" ]]; then
        printf 'Missing SDK component: %s\n' "$path" >&2
        printf 'Install platforms;android-35 and build-tools;35.0.0 first.\n' >&2
        exit 1
    fi
done
rm -rf "$build_dir"
mkdir -p "$build_dir/classes" "$build_dir/generated" "$build_dir/dex" "$output_dir" "$(dirname "$keystore_path")"

# Gradle supplies namespace/applicationId, while standalone AAPT requires a package
# in its input manifest. Generate that build-only manifest without changing source.
python3 - "$project_dir/app/src/main/AndroidManifest.xml" "$build_dir/AndroidManifest.xml" <<'PY'
import sys, xml.etree.ElementTree as ET
ET.register_namespace('android', 'http://schemas.android.com/apk/res/android')
tree = ET.parse(sys.argv[1])
tree.getroot().set('package', 'app.ticketwatch.mobile')
tree.write(sys.argv[2], encoding='utf-8', xml_declaration=True)
PY

"$tools_dir/aapt2" compile --dir "$project_dir/app/src/main/res" -o "$build_dir/resources.zip"
"$tools_dir/aapt2" link -o "$build_dir/resources.apk" \
    --manifest "$build_dir/AndroidManifest.xml" \
    -I "$platform_jar" --java "$build_dir/generated" \
    --min-sdk-version 26 --target-sdk-version 35 \
    --version-code 6 --version-name 0.6.0-test "$build_dir/resources.zip"
find "$project_dir/app/src/main/java" "$build_dir/generated" -name '*.java' -print > "$build_dir/sources.txt"
"$java_bin" com.sun.tools.javac.Main -encoding UTF-8 -source 17 -target 17 \
    -classpath "$platform_jar" -d "$build_dir/classes" @"$build_dir/sources.txt"
"$java_bin" sun.tools.jar.Main --create --file "$build_dir/classes.jar" -C "$build_dir/classes" .
"$tools_dir/d8" --min-api 26 --lib "$platform_jar" --output "$build_dir/dex" "$build_dir/classes.jar"
python3 - "$build_dir" <<'PY'
import pathlib, shutil, sys, zipfile
root = pathlib.Path(sys.argv[1])
shutil.copyfile(root / 'resources.apk', root / 'unsigned.apk')
with zipfile.ZipFile(root / 'unsigned.apk', 'a', compression=zipfile.ZIP_DEFLATED) as archive:
    for dex in sorted((root / 'dex').glob('*.dex')):
        archive.write(dex, dex.name)
PY
"$tools_dir/zipalign" -f -p 4 "$build_dir/unsigned.apk" "$build_dir/aligned.apk"
if [[ ! -f "$keystore_path" ]]; then
    "$keytool_bin" -genkeypair -keystore "$keystore_path" -storepass android \
        -keypass android -alias androiddebugkey -keyalg RSA -keysize 2048 \
        -validity 10000 -dname 'CN=Android Debug,O=Android,C=US' >/dev/null
    chmod 600 "$keystore_path"
fi
"$tools_dir/apksigner" sign --ks "$keystore_path" --ks-key-alias androiddebugkey \
    --ks-pass pass:android --key-pass pass:android \
    --v4-signing-enabled false \
    --out "$output_dir/ticket-watch-android.apk" "$build_dir/aligned.apk"
"$tools_dir/apksigner" verify --verbose "$output_dir/ticket-watch-android.apk"
(cd "$output_dir" && sha256sum ticket-watch-android.apk > ticket-watch-android.apk.sha256)
chmod 644 "$output_dir/ticket-watch-android.apk" "$output_dir/ticket-watch-android.apk.sha256"
printf 'APK ready: %s\n' "$output_dir/ticket-watch-android.apk"
