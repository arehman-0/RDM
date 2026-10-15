# Publishing the extension (Chrome Web Store and Edge Add-ons)

Chrome and Edge install extensions from outside their stores only in developer mode
("Load unpacked"). The installer and the app offer that as the manual path; the normal
path for users is a store listing. **Nothing has been uploaded yet.**

## Files here

| File | Use |
|---|---|
| `build-zip.mjs` | `cd extension && npm run store:zip` → `store/out/idm-replica-extension-<version>.zip`: fresh build, no `key`, no source maps, no tests |
| `listing.md` | Name, descriptions, category, images checklist |
| `privacy-policy.md` | Privacy policy text (host it at a public URL) |
| `permissions.md` | Justification for every permission; review result |

## Steps

1. One-time accounts (you do this; they are paid/verified and not created by any script):
   Chrome Web Store developer account; Microsoft Partner Center account for Edge.
2. `cd extension && npm run store:zip`.
3. Upload the zip, fill in the listing from `listing.md`, paste the justifications from
   `permissions.md`, link the hosted privacy policy, add screenshots.
4. Submit for review.

## After the first upload: paste the store IDs into the app

Each store assigns its **own** extension ID (Chrome and Edge differ, and both differ from
the development ID `pcmcmknffhfehgpkoedecnabbphgnabg`, which comes from the manifest
`key` the store zip leaves out). The app only accepts pairing from allowed IDs, so:

1. Copy the IDs from the dashboards (Chrome: the item ID in the URL /"Item ID"; Edge:
   the "Extension ID" on the product page, not the Store ID / product ID).
2. Edit `src-tauri/extension-store.json`:

   ```json
   {
     "TODO": "…",
     "chromeWebStoreId": "<32 letters a-p>",
     "chromeWebStoreUrl": "https://chromewebstore.google.com/detail/<slug>/<id>",
     "edgeAddonsId": "<32 letters a-p>",
     "edgeAddonsUrl": "https://microsoftedge.microsoft.com/addons/detail/<slug>/<product id>"
   }
   ```
3. `cargo test` (a test checks the file: IDs are 32 letters a–p, URLs are https), then
   rebuild the installers. The IDs become pre-allowed in the bridge
   (`bridge::shipped_extension_ids()`), and the "Open … page" buttons in the installer and
   the app are enabled. The dev ID stays allowed for "Load unpacked".

Until then the store buttons show "(not published yet)" and are disabled.

## Not done (needs you)

The real store flow (accounts, review, the IDs the stores assign, hosting the policy,
screenshots) was not exercised: no accounts were created and nothing was published.
