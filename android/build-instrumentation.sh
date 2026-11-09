#!/usr/bin/env bash
# Internal platform-only instrumentation build; never bundle it with the user APK.
set -euo pipefail
project_dir="$(cd "$(dirname "$0")" && pwd)"
sdk_dir="${ANDROID_SDK_ROOT:-${ANDROID_HOME:-/workspace/.android-sdk}}"
tools_dir="$sdk_dir/build-tools/35.0.0"
platform_jar="$sdk_dir/platforms/android-35/android.jar"
main_jar="$project_dir/build/manual/classes.jar"
build_dir="$project_dir/build/instrumentation"
output_dir="$project_dir/../dist"
keystore_path="${TICKET_ANDROID_DEBUG_KEYSTORE:-$project_dir/.signing/debug.keystore}"
java_bin="${JAVA_HOME:+$JAVA_HOME/bin/}java"
if [[ ! -f "$main_jar" || ! -f "$keystore_path" ]]; then
    printf 'Build the main APK first with ./build-apk.sh\n' >&2
    exit 1
fi
rm -rf "$build_dir"
mkdir -p "$build_dir/classes" "$build_dir/dex" "$output_dir"
python3 - "$project_dir/app/src/androidTest/AndroidManifest.xml" "$build_dir/AndroidManifest.xml" <<'PY'
import sys, xml.etree.ElementTree as ET
ET.register_namespace('android', 'http://schemas.android.com/apk/res/android')
tree = ET.parse(sys.argv[1])
tree.getroot().set('package', 'app.ticketwatch.mobile.test')
tree.write(sys.argv[2], encoding='utf-8', xml_declaration=True)
PY
"$tools_dir/aapt2" link -o "$build_dir/resources.apk" \
    --manifest "$build_dir/AndroidManifest.xml" -I "$platform_jar" \
    --min-sdk-version 26 --target-sdk-version 35 --version-code 6 --version-name 0.6.0-test
find "$project_dir/app/src/androidTest/java" -name '*.java' -print > "$build_dir/sources.txt"
"$java_bin" com.sun.tools.javac.Main -encoding UTF-8 -source 17 -target 17 \
    -classpath "$platform_jar:$main_jar" -d "$build_dir/classes" @"$build_dir/sources.txt"
"$java_bin" sun.tools.jar.Main --create --file "$build_dir/classes.jar" -C "$build_dir/classes" .
"$tools_dir/d8" --min-api 26 --lib "$platform_jar" --classpath "$main_jar" \
    --output "$build_dir/dex" "$build_dir/classes.jar"
python3 - "$build_dir" <<'PY'
import pathlib, shutil, sys, zipfile
root = pathlib.Path(sys.argv[1])
shutil.copyfile(root / 'resources.apk', root / 'unsigned.apk')
with zipfile.ZipFile(root / 'unsigned.apk', 'a', compression=zipfile.ZIP_DEFLATED) as archive:
    for dex in sorted((root / 'dex').glob('*.dex')):
        archive.write(dex, dex.name)
PY
"$tools_dir/zipalign" -f -p 4 "$build_dir/unsigned.apk" "$build_dir/aligned.apk"
"$tools_dir/apksigner" sign --ks "$keystore_path" --ks-key-alias androiddebugkey \
    --ks-pass pass:android --key-pass pass:android \
    --v4-signing-enabled false \
    --out "$output_dir/ticket-watch-tests.apk" "$build_dir/aligned.apk"
"$tools_dir/apksigner" verify "$output_dir/ticket-watch-tests.apk"
printf 'Internal test APK ready: %s\n' "$output_dir/ticket-watch-tests.apk"
