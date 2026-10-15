# Code signing (Windows) — options and how to turn it on

**Status: not signed.** The installers built by `scripts/build-installer.ps1` and CI are
unsigned. Windows SmartScreen therefore shows *"Windows protected your PC"* on the first
run (users click **More info → Run anyway**), and some antivirus products are stricter
with unsigned downloads. Nothing below is enabled; no paid service is used.

## What gets signed

Three files: the app exe (`IDM Replica.exe`, inside the installer), the uninstaller
(`uninstall.exe`, generated during the NSIS build) and the installer itself. Tauri v1's
bundler signs all three with `signtool` when `tauri.bundle.windows.certificateThumbprint`
is set: the exe before packing, the uninstaller through NSIS `!uninstfinalize`, then the
installer.

## Options

| | Azure Trusted Signing (Artifact Signing) | SignPath Foundation (open source) | OV / EV certificate (commercial CA) |
|---|---|---|---|
| Cost | Pay-as-you-go monthly plan (a few USD/month) | Free for qualifying OSS projects | Roughly USD 200–600/year; EV usually more |
| Who can get it | Organisations, and individuals in supported countries, after Microsoft identity validation | Open-source projects that meet their policy (public repo, OSI licence, reproducible CI build) | Anyone who passes the CA's validation |
| Key storage | In Azure (no hardware token) | In SignPath's HSM; you never hold the key | Since 2023 always on a hardware token or cloud HSM |
| SmartScreen | Builds reputation; Microsoft-issued, short-lived certs | Builds reputation under "SignPath Foundation" as publisher | OV builds reputation over downloads; EV no longer skips that (since 2024) |
| CI fit | Good: GitHub OIDC → Azure, `signtool` + Trusted Signing dlib | Good: GitHub Action submits the artifact, receives it signed | Hard with a USB token; fine with a cloud-HSM CA |
| Publisher shown | Your validated name | "SignPath Foundation" (on behalf of the project) | Your validated name |

**Recommendation:** apply to SignPath Foundation first (free, MIT-licensed public repo,
the CI build already exists). If it is not accepted, or you want your own name as
publisher, use Azure Trusted Signing. A commercial certificate on a hardware token is the
most work for CI.

## Turning it on

### A. A certificate in the Windows certificate store (OV/EV, cloud-HSM CAs)

Signing is behind an env flag in `scripts/build-installer.ps1`:

```powershell
$env:IDM_SIGN = '1'
$env:IDM_CERT_THUMBPRINT = '<SHA-1 thumbprint of the code-signing cert>'
$env:IDM_TIMESTAMP_URL = 'http://timestamp.digicert.com'   # or your CA's RFC 3161 URL
powershell -ExecutionPolicy Bypass -File scripts\build-installer.ps1 -Variant lite
```

which adds to the variant config:

```json
{ "tauri": { "bundle": { "windows": {
  "certificateThumbprint": "<thumbprint>",
  "digestAlgorithm": "sha256",
  "timestampUrl": "http://timestamp.digicert.com"
} } } }
```

CI (GitHub Actions, Windows job), with the PFX only for cloud-HSM/legacy setups:

```yaml
      - name: Import signing certificate
        if: env.IDM_SIGN == '1'
        env:
          PFX_BASE64: ${{ secrets.WINDOWS_PFX_BASE64 }}
          PFX_PASSWORD: ${{ secrets.WINDOWS_PFX_PASSWORD }}
        run: |
          $pfx = Join-Path $env:RUNNER_TEMP cert.pfx
          [IO.File]::WriteAllBytes($pfx, [Convert]::FromBase64String($env:PFX_BASE64))
          $c = Import-PfxCertificate -FilePath $pfx -CertStoreLocation Cert:\CurrentUser\My `
                 -Password (ConvertTo-SecureString $env:PFX_PASSWORD -AsPlainText -Force)
          "IDM_CERT_THUMBPRINT=$($c.Thumbprint)" >> $env:GITHUB_ENV
          Remove-Item $pfx
```

and set `IDM_SIGN: '1'` in the job's `env`.

### B. Azure Trusted Signing / SignPath (no thumbprint)

Tauri v1 has no custom sign-command hook (v2 has `signCommand`), so these sign *after*
the build. The app exe is inside the installer, so the order is:

1. `cargo build --release --features custom-protocol` (or `tauri build` once), then sign
   `src-tauri\target\release\idm-replica.exe` in place:
   ```powershell
   signtool sign /v /fd SHA256 /tr http://timestamp.acs.microsoft.com /td SHA256 `
     /dlib "<path>\Azure.CodeSigning.Dlib.dll" /dmdf metadata.json src-tauri\target\release\idm-replica.exe
   ```
   (`metadata.json`: `{"Endpoint": "...", "CodeSigningAccountName": "...", "CertificateProfileName": "..."}`;
   CI logs in with `azure/login` using OIDC, no stored password.)
2. Run `tauri build` again: cargo has nothing to rebuild, so the signed exe is packed.
   To also sign the uninstaller, put the same `signtool` command into the template's
   `UNINSTALLERSIGNCOMMAND` define (`src-tauri/installer/installer.nsi`; `%1` is the file).
3. Sign the finished installer in `release\` the same way, then regenerate
   `SHA256SUMS.txt` (signing changes the hash).

For SignPath, steps 1 and 3 become "upload artifact → SignPath GitHub Action → download
signed artifact".

**Check after signing:** `Get-AuthenticodeSignature release\*.exe` must say `Valid`, and the
exe inside a fresh install too.

## Not verified

No certificate was available, so none of the signing paths above has been run. The
`IDM_SIGN` hook only adds the config shown; it fails fast if no thumbprint is given.
