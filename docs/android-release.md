# Android release signing

GitHub's Android release job needs a persistent signing key. Use the same key for every release so Android can install updates over the previous version.

Create a key locally and keep a secure backup outside the repository:

```sh
keytool -genkeypair -v -keystore denki-release.jks -keyalg RSA -keysize 2048 -validity 10000 -alias denki
base64 -w 0 denki-release.jks
```

Add these repository Actions secrets in GitHub. Use the base64 output for the first one and the values chosen during key creation for the others:

- `ANDROID_KEYSTORE_BASE64`
- `ANDROID_KEYSTORE_PASSWORD`
- `ANDROID_KEY_ALIAS`
- `ANDROID_KEY_PASSWORD`

Never commit the keystore or its passwords. Losing the key prevents existing Android installs from receiving signed updates.
