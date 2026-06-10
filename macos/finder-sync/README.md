# DropBeam Finder Sync extension (file badges, like LucidLink)

Draws a badge + a **"From &lt;name&gt;"** context-menu line on files that arrived in a
shared folder, so you can see at a glance who sent each one — in Finder itself,
the way LucidLink and Dropbox do it.

## How it works

1. The main app stamps every received file with a `com.dropbeam.from` extended
   attribute (the sender's name) — see `src-tauri/src/provenance.rs`. This already
   ships and works today (visible via `xattr -p com.dropbeam.from <file>` and in
   Finder's "Get Info" once the extension is active).
2. The main app publishes the list of shared-folder paths to
   `~/Library/Application Support/com.dropbeam.app/finder-folders.json`
   (see `SyncManager::write_finder_folders`).
3. This extension (`DropBeamFinderSync.swift`) watches those folders, reads the
   xattr per file, and badges it. Verified to typecheck against the real
   `FinderSync.framework` SDK.

## Building / embedding

`build-and-embed.sh` compiles the Swift to a `.appex` and embeds it in a built
`DropBeam.app`, then code-signs. It is **not yet wired into the release CI** — see
below.

## The one real requirement: code signing

A Finder Sync extension is a separate **app extension** bundled in the app. For it
to load on **other people's Macs** (your shared-folder peers), the app +
extension must be signed with an **Apple Developer ID** and **notarized**. That
needs a paid Apple Developer account ($99/yr) enrolled under your Apple ID — which
only the project owner can set up.

- **With a Developer ID:** set `SIGN_ID` + notarize → badges work for everyone.
- **Ad-hoc (current `"signingIdentity": "-"`):** the app installs but a Finder
  extension won't reliably register/load on a downloaded build; at best it loads
  on *your own* machine after you enable it in System Settings → Login Items &
  Extensions → Finder.

So the remaining steps are a project decision, not a code problem:
1. Enroll in the Apple Developer Program, create a "Developer ID Application" cert.
2. Set the cert in `tauri.conf.json` (`bundle.macOS.signingIdentity`) and add the
   notarization step to CI.
3. Add a CI step after `tauri build` that runs `build-and-embed.sh` with the cert.

Until then the provenance **data** (the xattr) and the in-app "received from
&lt;name&gt;" notification already give you who-sent-what; the Finder badge lights up
once signing is in place.
