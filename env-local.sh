# The build environment from what is already on this computer (no downloads): the SDK Android Studio left in
# ~/Android/Sdk (it runs through nix-ld), a JDK and zip from the Nix store, Rust from rustup.
#   source ./env-local.sh && ./build.sh --install
sdk="${ANDROID_HOME:-$HOME/Android/Sdk}"
export ANDROID_HOME="$sdk" ANDROID_SDK_ROOT="$sdk"
export ANDROID_NDK_ROOT="$(ls -d "$sdk"/ndk/* | sort -V | tail -1)"
export BUILD_TOOLS="$(ls -d "$sdk"/build-tools/* | sort -V | tail -1)"
export ANDROID_JAR="$sdk/platforms/android-34/android.jar"
# the first JDK of the store whose tools start (a partly fetched one may lack its libraries); apksigner wants Java 11+
jdk=""
for j in /nix/store/*-openjdk*-2[0-9]*/bin /nix/store/*-openjdk*-1[7-9]*/bin; do
    if [ -x "$j/java" ] && [ -x "$j/keytool" ] && "$j/java" -version >/dev/null 2>&1 && "$j/keytool" -help >/dev/null 2>&1; then jdk="$j"; break; fi
done
zipbin="$(ls -d /nix/store/*-zip-3.0/bin 2>/dev/null | head -1)"
export PATH="$HOME/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin:$sdk/platform-tools:$jdk:$zipbin:$PATH"
