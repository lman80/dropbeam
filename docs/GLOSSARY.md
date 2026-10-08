# DropBeam glossary (desktop UI copy)

One name for each thing, used the same way on every screen.

## Things

| Use | For | Don't say |
| --- | --- | --- |
| **Friends** | People you've added (one entry per person, however many devices they have) | contacts, peers |
| **My devices** / "Your iPhone", "Your Mac" | Your own linked devices | "this account's devices", raw device names when a label exists |
| **Send & Receive** | The page for one-off transfers | Transfers page |
| **Quick Send** | A send to anyone via a code or QR code | Direct ticket, link |
| **Shared Folders** | Folders that stay in sync with friends | synced folders, pairs, drop folders |
| **Locations** | Folders a friend lets you browse/upload to on their device or NAS | hosts, gateways |
| **Synced folder** (inside Locations) | A folder on this device kept copied to a Location | mirror, backup |
| **Transfer Server** | A computer that holds sends while people are offline | mailbox, relay box |
| **Recoverable Files** | Saved copies of deleted/replaced shared-folder files (History → Recoverable Files; every folder's ••• menu) | trash, archive, history copies, Folder History |
| **Fully synced** | Shared-folder mode where adds, changes **and deletes** reach everyone | Total sync, mirror |
| **Add and edit only** | Shared-folder mode where deletes only remove your own copy | Two-way |
| **Only I can change it** | Shared-folder mode where others get a copy that follows yours | One-way, read-only copy |
| **Editor** / **Viewer** | Can add, change and delete files / can open and copy files, but not change them | owner/peer roles, read-only |
| **Unfinished transfers** | Partly received files kept so a transfer can pick up where it stopped | transfer leftovers, transfer cache, partials |
| **Relay** | The encrypted fallback path when devices can't connect directly (say "slower, still encrypted" where it needs explaining; connection tuning lives in Settings → Advanced) | canary, DERP |

## Actions

| Use | Not |
| --- | --- |
| **Stop Sharing…** (a Shared Folder with one person, or a Location you host) | Unpair, Remove share |
| **Leave Folder…** (a Shared Folder with several people) | Unpair, Exit group |
| **Stop Syncing…** (a synced folder) | Remove sync |
| **Remove Friend…** / **Remove Device…** | Delete friend, Unfriend |
| **Send Files…** (main window and menu bar alike) | Send a file, Beam |
| **Receive…** | Have a code? |
| **Join a Folder…** (desktop) · **Join a Shared Folder** (iOS) | Accept invite, Accept a Folder Invite |
| **Check Everything Matches** (shared folder) · **Check the Copy** (a sent transfer) | Verify, Verify Copy |
| **Show in Finder / File Explorer / Folder** on every received file — a labelled button, never icon-only | |
| **Show in Finder** (macOS) · **Show in File Explorer** (Windows) · **Show in Folder** (Linux) | "Show in Finder" on every OS |
| **Try Again** | Retry (except the compact Retry on a failed transfer) |

## Capitalisation

- **Title Case** for buttons, menu items, tabs and window/dialog titles that are names
  ("Send Files…", "Add Person…", "New Shared Folder").
- **Sentence case** for everything else: section headers, row labels, descriptions,
  placeholders, toasts, and dialog titles phrased as questions ("Stop sharing “Photos”?").
- An ellipsis (…) only when the control opens something that needs more input.

## Everyday words

- Explain a feature in one sentence where it's used ("A shared folder is the same folder on everyone's computer").
- Say what happens to files **before** a destructive or delete-syncing change, in the confirm itself.
- Never in primary UI: pair, mirror, endpoint, reconcile, ticket, manifest, peer, UDP, parallel streams. They may appear only in Settings → Advanced or behind **Details**.
- A refused permission always comes with where to fix it (and an **Open Settings** button when the OS allows).

## Dates, sizes, errors

- Dates: "Just now", "5 min ago", "Today 7:25 PM", "Yesterday 7:25 PM", "Oct 2" (year only
  outside the current one) — `src/lib/dates.ts`.
- Sizes: Finder style, "640 KB", "14.2 GB"; live counters keep fixed decimals — `src/lib/format.ts`.
- Errors: one plain sentence; the raw text goes behind **Details** — `src/lib/errors.ts`.
