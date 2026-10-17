# easel-debug.keystore: DEBUG ONLY, never for Play

`easel-debug.keystore` is a fixed, publicly known signing key (store password `easel-debug`, alias
`easeldebug`, key password `easel-debug`). It exists so every debug build and every debug-signed
release APK has the same signature and installs over the previous one (otherwise each machine signs
with its own `~/.android/debug.keystore` and Android answers `INSTALL_FAILED_UPDATE_INCOMPATIBLE`).

Anyone can sign an APK with it, so it proves nothing. Never upload an APK signed with it to Google Play
or any store; the real upload key stays with the maintainer (`-Peskiz.keystore=...`). Generated with:

    keytool -genkeypair -keystore easel-debug.keystore -storetype PKCS12 -storepass easel-debug \
      -keypass easel-debug -alias easeldebug -keyalg RSA -keysize 2048 -validity 36500 \
      -dname "CN=Eskiz DEBUG ONLY - never for Play, O=Eskiz"
