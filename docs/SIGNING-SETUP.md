# Code signing setup (owner steps)

`.github/workflows/release.yml` signs the macOS and Windows builds **only when the secrets below exist**. Without them, releases still build: macOS is ad-hoc signed and not notarized, so Gatekeeper shows "Open Anyway"; Windows is unsigned, so SmartScreen shows "Run anyway". You can add either platform on its own.

Why it matters: a notarized Developer ID app opens without warnings. Its **code identity also stays the same across updates**, so macOS keeps the Local Network, Files & Folders and Login Item permissions after each update. Ad-hoc builds can lose them and ask again.

Status on 2026-10-04: team **R2RDA8476R** has *Apple Development* and *Apple Distribution* certificates but **no Developer ID Application certificate**. (Checked read-only with the App Store Connect API and the local keychain. Nothing was created or revoked.)

---

## macOS: Developer ID + notarization

### 1. Create the Developer ID Application certificate (Account Holder only)

Apple lets only the **Account Holder** create Developer ID certificates. The App Manager API key can't do it.

1. On this Mac, open **Keychain Access → Certificate Assistant → Request a Certificate From a Certificate Authority…**. Use your email and the name "DropBeam Developer ID", choose **Saved to disk**, and save `DeveloperID.certSigningRequest`.
2. Go to <https://developer.apple.com/account/resources/certificates/add>, choose **Developer ID Application** (profile type *G2 Sub-CA*), and upload the CSR. Download `developerID_application.cer` and double-click it to add it to your login keychain.
3. Check that it's there:
   ```bash
   security find-identity -v -p codesigning | grep "Developer ID Application"
   # → "Developer ID Application: ASHTON MICHAEL MILLER (R2RDA8476R)"
   ```

### 2. Export it as a .p12

In Keychain Access → **My Certificates**, right-click **Developer ID Application: … (R2RDA8476R)** (expand it and make sure the private key is included) → **Export…** → `DeveloperID.p12`. Set a strong password.

### 3. Store the secrets in GitHub

```bash
cd ~/DropBeam   # any checkout of lman80/dropbeam
# The certificate (base64, one line) and its export password
base64 -i DeveloperID.p12 | tr -d '\n' | gh secret set APPLE_CERTIFICATE -R lman80/dropbeam
gh secret set APPLE_CERTIFICATE_PASSWORD -R lman80/dropbeam          # paste the .p12 password
gh secret set APPLE_SIGNING_IDENTITY -R lman80/dropbeam \
  --body "Developer ID Application: ASHTON MICHAEL MILLER (R2RDA8476R)"   # exact name from step 1.3

# Notarization: the existing App Store Connect API key works (App Manager is enough for notarytool)
gh secret set APPLE_API_KEY    -R lman80/dropbeam --body "WZCJYW5KT9"
gh secret set APPLE_API_ISSUER -R lman80/dropbeam --body "02b71951-6d9c-4619-937b-4e3cd35ec60b"
gh secret set APPLE_API_PRIVATE_KEY -R lman80/dropbeam < ~/.private_keys/AuthKey_WZCJYW5KT9.p8
```

Then delete the local `DeveloperID.p12` (or keep it in a password manager). Store the .p12 password in the macOS Keychain like the other keys:
`security add-generic-password -U -a "$USER" -s claude.apple.developer-id-p12 -l "DropBeam Developer ID .p12 password" -w '<password>'`.

### 4. Check a signed build

On the next tag, the macOS job log should say "Developer ID signing + notarization enabled." To check the downloaded app:

```bash
spctl -a -vvv -t install /Applications/DropBeam.app      # → accepted, source=Notarized Developer ID
codesign -dv --verbose=4 /Applications/DropBeam.app 2>&1 | grep -E "Authority|TeamIdentifier|flags"
xcrun stapler validate /Applications/DropBeam.app
```

Notes:
- Tauri turns on the hardened runtime when it signs with a real identity. `src-tauri/Entitlements.plist` keeps the camera entitlement (QR scanning). No sandbox entitlements are needed because DropBeam isn't sandboxed on macOS.
- Users on old ad-hoc builds update normally. Because the signing identity changes, macOS may ask **once** for Local Network / folder access after the first signed update. After that, permissions stay put.

---

## Windows: Azure Trusted Signing

Trusted Signing (about US$10/month) gives SmartScreen reputation right away. A traditional OV certificate also works, but it needs an HSM/token, which doesn't fit CI as well.

### 1. Set it up in Azure (one time)

1. Create an Azure account and subscription at <https://portal.azure.com>.
2. Create a **Trusted Signing account**: search "Trusted Signing" → Create. Note the region endpoint (e.g. `https://eus.codesigning.azure.net`) and the account name.
3. In it, do **Identity validation → Individual** (or Organization if you have a registered business) and wait for approval. Individual validation needs a government ID.
4. Create a **Certificate profile** of type *Public Trust* and note its name.
5. Create an **App registration** (Microsoft Entra ID → App registrations → New) named `dropbeam-ci`, then add a **client secret** and note the *Application (client) ID*, the *Directory (tenant) ID* and the secret value.
6. In the Trusted Signing account → **Access control (IAM)** → Add role assignment → **Trusted Signing Certificate Profile Signer** → assign it to `dropbeam-ci`.

### 2. Store the secrets in GitHub

```bash
gh secret set AZURE_CLIENT_ID          -R lman80/dropbeam   # Application (client) ID
gh secret set AZURE_CLIENT_SECRET      -R lman80/dropbeam   # client secret value
gh secret set AZURE_TENANT_ID          -R lman80/dropbeam   # Directory (tenant) ID
gh secret set TRUSTED_SIGNING_ENDPOINT -R lman80/dropbeam --body "https://eus.codesigning.azure.net"
gh secret set TRUSTED_SIGNING_ACCOUNT  -R lman80/dropbeam --body "<account name>"
gh secret set TRUSTED_SIGNING_PROFILE  -R lman80/dropbeam --body "<certificate profile name>"
```

On the next tag, the Windows job installs `trusted-signing-cli` and passes Tauri a `bundle.windows.signCommand`. Tauri then signs `DropBeam.exe`, the NSIS installer and the MSI. To check: right-click the installer → Properties → **Digital Signatures**.

---

## Updater signing (already set up)

`TAURI_SIGNING_PRIVATE_KEY` / `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` sign the update bundles for the in-app updater (public key in `tauri.conf.json → plugins.updater.pubkey`). These secrets already exist. **Don't rotate them**, or every installed copy stops accepting updates.

## Checklist

| Secret | Platform | Required for |
|---|---|---|
| `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY` | macOS | Developer ID signing |
| `APPLE_API_KEY`, `APPLE_API_ISSUER`, `APPLE_API_PRIVATE_KEY` | macOS | notarization (needs the three above) |
| `AZURE_CLIENT_ID`, `AZURE_CLIENT_SECRET`, `AZURE_TENANT_ID`, `TRUSTED_SIGNING_ENDPOINT`, `TRUSTED_SIGNING_ACCOUNT`, `TRUSTED_SIGNING_PROFILE` | Windows | Authenticode signing |
| `TAURI_SIGNING_PRIVATE_KEY`, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | all | updater (exists) |

Linux packages aren't code-signed. The updater signature covers AppImage updates.
