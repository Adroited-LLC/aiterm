# Iroh Android 1.1.0 — 16 KB ARM64 rebuild

`iroh-android-1.1.0-page16-arm64.aar` contains the Android initializer and
ARM64 `libiroh_ffi.so` from the official `n0-computer/iroh-ffi` `v1.1.0` tag
(`5e451092dba0c1a09ee83ff6e5be37b1152a5c58`). The native library was rebuilt
with the upstream 16 KB page-size fix from commit `c948937`:

```toml
[target.aarch64-linux-android]
rustflags = ["-C", "link-arg=-Wl,-z,max-page-size=16384"]
```

Build command (NDK 27.0.12077973, cargo-ndk 4.1.2):

```sh
cargo ndk -o android-jniLibs -t arm64-v8a build --release --lib -p iroh-ffi
```

The library is stripped with the NDK's `llvm-strip`. The AAR SHA-256 is
`20743be3f6853987782efa2373ec87634a3e7b03609e63a53acce963f442b421`.
Replace this file with the first upstream Maven release that includes the
same fix.
