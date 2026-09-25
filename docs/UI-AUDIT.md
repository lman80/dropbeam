# DropBeam desktop — UI audit (2026-09-25)

Scope: every desktop surface (main window, menu-bar popover, HUD, receive card), light and dark, at the default 980×700 window and the 760×560 minimum. Screenshots were taken from the browser preview with the seeded mock (`src/lib/mock.ts`, `?empty=1` for empty states). The before/after galleries live outside the repo, in the UI-polish scratch directory.

Severity: **P1** reads as broken or unprofessional at a glance. **P2** is a visible craft issue. **P3** is polish.

---

## 0. System-wide (applies to every screen)

1. **P1 — Gradients everywhere.** The primary buttons, toggles, progress bars, the percentage text (`.gradient-text`), the sent chat bubbles, the logo tile, avatars and the window background (two radial glows plus a linear wash) all use violet→purple gradients. The result reads as generated rather than native.
2. **P1 — Glow shadows.** Primary buttons carry a coloured 4–18px glow, and the drop-zone icon tile has a 28px violet glow. No native control glows.
3. **P1 — An explanatory subtitle under every page title** ("Beam files straight to a friend…", "Folders that stay in sync with friends, automatically.", "Your friends’ folders, within reach." …). These add noise and no information.
4. **P1 — Pill badges on everything:** *Local network / Direct / Relay / Connecting*, *Total sync*, *Two-way*, *View only (you receive)*, *Up to date*, *Paused*, *Gateway*, and Editor/Viewer segments inside member pills. Semantic colour is spent on non-status information.
5. **P1 — Internal state leaks into primary UI:** raw IP:port peers ("Received from 192.168.1.40:5", "Sent to 70.2.1.9:5"), RTT ("· 3 ms", "88 ms"), relay region codes ("Relay · use1"), "upgrading to direct…", "Saved, unverified", "checking link…", SHA digests, absolute paths (`/Users/you/…`, `/Volumes/…`).
6. **P1 — Button soup.** Rows of 3–6 outlined, labelled buttons per card (Locations: Sync now / Pause / Open folder / Remove; Friends: toggle + Invite + … + trash; folder settings: History / Verify / Show invite / Unpair).
7. **P2 — Type scale is loose and oversized.** The page titles are 22px/750, the dialog titles 18px/750, the nav items 14.5px/600, and body text 13.5px. There are 8 sizes plus many ad-hoc `calc(12px…)`, `calc(12.5px…)` and `calc(13px…)` inline values. Weights of 650/700/750 are used interchangeably.
8. **P2 — Radii are inconsistent:** 6/7/8/9/10/11/12/14/16/20 and pills. Cards use 16px, dialogs 20px, buttons 10px and inputs 10px.
9. **P2 — Cards inside cards, borders on everything.** Examples: the Settings → Devices list inside a card, the member pills inside folder cards, the queued rows inside a card, and the QR tile inside a card.
10. **P2 — Surfaces all carry `--shadow` (8–24px blur),** so the flat page looks busy. On macOS, content lists sit flat on the window background.
11. **P2 — Emoji in UI copy and data previews:** "📎 Mountains.jpg" in the chat list, "📎 File", "🎞️ GIF", the ★ on "100 ★", and the emoji-heavy placeholder copy.
12. **P2 — Inline styles dominate** (hundreds of `style={{…}}`). Spacing therefore follows no rhythm: 2/3/5/6/7/9/10/11/13/14/18/22px all occur.
13. **P2 — Focus handling.** `.btn`/`.icon-btn` use a focus ring shadow while the nav/menus use an outline, so there are two different ring styles. `:focus` (not `:focus-visible`) styles on some rows leave a stuck highlight after a mouse click (see Sidebar → Feedback).
14. **P2 — Bouncy motion.** Framer spring `stiffness 380–400` scale-ins on cards, dialogs, toasts and the drop zone (it scales to 1.012 on hover). Every page also fades and slides in by 6px on each navigation, so the layout shifts on every tab switch.
15. **P3 — Icon strokes and sizes vary** from 11 to 30px in the same contexts. Lucide stroke 2 is heavier than SF Symbols at these sizes.
16. **P3 — Dark mode** is a dark-navy tinted theme (#0c0d15, #16171f) with a blue cast rather than neutral system greys. Secondary buttons are near-invisible dark-on-dark pills.

## 1. Shell — title bar, sidebar, banners, onboarding, toasts, loading

1. **P1 — The title bar duplicates the app identity:** a gradient logo tile plus "DropBeam" next to the traffic lights. macOS apps don't title their main window with the app name when the sidebar already brands it.
2. **P1 — An unexplained monitor icon top-right** (the appearance cycler system→light→dark) with no label; its icon doesn't read as "appearance". It duplicates Settings → Appearance.
3. **P1 — Sidebar "Feedback"** sits in the main navigation as if it were a page. It keeps a highlighted background after it opens the panel (focus/hover state stuck), so it reads as selected alongside the real selection.
4. **P2 — Sidebar selection** uses accent-soft background + accent text; hover uses a 5% tint that is almost the same, so selected and hover are hard to tell apart. Items are 36px tall with 14.5px/600 text — heavier than Finder/Mail (13px regular, 28px rows).
5. **P2 — The sidebar count badges** are accent filled circles at 20px. In the icon rail (≤900px) they overlap the icon by half its width.
6. **P2 — The icon rail at the minimum size:** the title-bar brand stays at x=80+, misaligned with the 64px rail. The profile footer shows only an avatar with no affordance.
7. **P2 — The profile footer** shows the *device* name ("Ashton's MacBook …", truncated) with "This device" beneath. The avatar uses the gradient. It is not clickable (no route to profile/settings).
8. **P2 — Banners (Local Network / install location)** are amber full-width strips with bold inline text and a ghost button. The text wraps to two lines at 980px and the × is misaligned with the text baseline.
9. **P2 — Onboarding name prompt:** a centred card with a gradient logo tile, a two-sentence explainer, a pre-filled *device* name ("Ashton's MacBook Pro") as the person's name, a gradient "Continue" and a link-style secondary. It's generic, and the pre-fill is wrong (a device name is not what "people call you").
10. **P2 — Toasts** carry a coloured left border strip plus a coloured icon plus a card shadow (a "notification card" look). Long errors wrap to 3+ lines at 380px. There is no pattern for an inline action (e.g. Retry).
11. **P3 — The loading splash** is a pulsing gradient logo in the middle of a gradient background.

## 2. Send & Receive

1. **P1 — A generic "icon tile + dashed drop zone" hero:** a 68px gradient tile with glow, a 2px dashed border, "Drag files here to send / or click to choose files & folders", and a scale-up on hover. This is the template look the brief calls out.
2. **P1 — Two small outlined pill buttons** float centred under the drop zone ("Have a code? Receive files", "Scan a QR code"); they're disconnected from anything.
3. **P1 — The empty state** is a 5-line paragraph including a "Tip:" sentence, under an icon tile.
4. **P1 — The receive-by-code form** replaces the two pills with an inline row: monospace input, "Scan QR code", a gradient "Receive" and "Cancel". It wraps at narrow widths.
5. **P1 — Transfer cards** stack a status tile, filename, status line, a locality/ConnInspector pill with RTT, then pause and × icons. Each state then adds its own block: the progress "62%" in gradient text, a thick 8px bordered bar, "live/avg" meter buttons, a red error well, a grey paused well with a second bar, "Transfer canceled." as a whole row, "Saved, unverified", and "Verify copy".
6. **P1 — Completed cards** show "Saved, unverified" under "Saved · 42.0 MB" and the summary "<1s · 42.0 GB/s avg", which looks like a bug.
7. **P1 — The failed card** shows only the red error box and a gradient "Retry". The title row says just "Failed", so there are two statuses for one state.
8. **P2 — Quick Send waiting card:** QR in a white tile with a grey frame, plus a helper paragraph, a monospace code field, a gradient "Copy code", a spinner line and a "Scan with DropBeam on your phone" caption — five elements for one action.
9. **P2 — The Accept/Decline offer** uses two full-width buttons (gradient + ghost), each 50% wide, under a sentence that repeats the title and status.
10. **P2 — The meters** "88.0 MB/s live", "33s left · avg", "calculating…" and "stalled" use jargon (live/avg).
11. **P2 — Parked state:** an amber well "Waiting for a direct connection" plus the relay chip with "upgrading to direct…" plus "Send over relay anyway". Three signals say the same thing.
12. **P2 — Card entrance** uses a spring with y+scale, and the list re-flows (layout animation) on every progress tick of any card.
13. **P3 — Choose a folder** (Windows/Linux) is a third floating pill.

## 3. Friends

1. **P1 — Every friend is a two-row card:** avatar, name, a ✎ pencil next to every name, presence, a ConnInspector pill with RTT, a ⟳ re-probe icon, a "◎ Check" link, a message icon, a gradient "Send" button. The second row holds the auto-accept toggle, "Approve files first", "Invite", "…" and a trash can. That's 8 controls per row times N friends.
2. **P1 — "checking link… ⟳ Check" status text** and RTT ("3 ms", "88 ms · upgrading to direct…") in primary UI.
3. **P1 — The You card** holds a 168px QR in a white tile with a grey frame, beside a paragraph of instructions, a monospace code field and a gradient "Copy code". It dominates the page, and every visit starts with your own QR.
4. **P2 — My devices** duplicates the friend-card design ("Your iPhone" with Send, Invite, trash) under an eyebrow with a "+ Link a device" quiet button. The empty state is a dashed call-to-action row.
5. **P2 — Inline "Invite" expands a second QR block** inside the card (cards inside cards).
6. **P2 — The remove confirmation** swaps the row's buttons for "Cancel / Remove" and adds a paragraph under the card; the layout jumps.
7. **P2 — The rename** is an inline input at 14.5px/650 that doesn't match the name's size, so the row jumps.
8. **P2 — Presence dot** (13px with a 2.5px ring) sits top-right of the avatar, while the device badge sits bottom-right. That's two overlays on a 44px avatar.
9. **P2 — The empty state** ("No friends yet") paragraph promises "it survives app updates, so you never re-add anyone" — this is support copy, not UI copy.
10. **P2 — Add friend dialog:** gradient icon tile in the header, subtitle and a 4-line help paragraph (with an arrow path "Friends → You"). The "Scan QR code" button floats above the textarea, and the full-width gradient primary sits below.
11. **P3 — The long name** (Maximilian …) truncates correctly, but the pencil icon then abuts the ellipsis.

## 4. Chat

1. **P1 — The file bubble defects (brief #1):**
   - Sent bubbles are a loud violet gradient, and the file card inside is purple-on-purple.
   - The sent video renders a large black `<video>` element with native controls at 440px wide and no poster/thumbnail until metadata loads.
   - Under the file card: "Delivered" + a locality pill ("Local network") + "· 3 ms" + "Saved, unverified" — the grey text is nearly invisible on violet. Then "✓ Delivered" repeats below the bubble.
   - The failed file shows red "Alex went offline…" on violet (unreadable) and a pink full-width "Retry" inside the bubble.
   - The in-progress file shows "Sending" + a locality pill + RTT, then "40%", "1.92 GB / 4.80 GB", a gradient bar, "88.0 MB/s", "33s left" — all inside the bubble on violet.
   - The multi-file message just reads "4 files 58.0 MB" with no list and no way to see which files.
2. **P1 — The thread doesn't stay anchored at the bottom** when images/video load after open: the newest messages end up below the fold (the ResizeObserver runs only on mobile).
3. **P1 — Day dividers repeat** ("Today" appears three times in one thread) because a divider is inserted after any 30-minute gap but only prints the day.
4. **P1 — The header** shows "Online now · Local network · 3 ms". "Shared folder" is an outlined labelled button and "…" a safety menu; the search is a bare icon with a custom tooltip.
5. **P2 — Conversation list:** 244px pane with a 22px bold "Chat" title, 44px gradient avatars, and preview text including "📎 Mountains.jpg". Unread counts are accent pills. There are no timestamps in the list. The selected row uses the accent-soft tint, the same as the hover. Friends with no messages ("No messages yet") pad out the list.
6. **P2 — Received bubbles** are white cards with a border and shadow on a tinted background. The link text is underlined accent inside them.
7. **P2 — Hover actions** (react / reply / more) float as a pill with its own shadow and overlap the time gutter. The reaction tray is a second floating pill with 7 emoji. The time gutter "4:32 PM" appears on hover only on the far right, far from the bubble.
8. **P2 — Reactions** show as bordered pills under the bubble, offset from its edge.
9. **P2 — "Read" / "Delivered"** status appears under deleted messages ("This message was deleted" · ✓✓ Read).
10. **P2 — The reply quote** renders as a separate grey line with a bar *above* the bubble, detached from it. The reply bar in the composer has a "Replying to Alex" label plus quoted text plus ×.
11. **P2 — The composer:** paperclip + emoji icons + a textarea with its own border + a gradient square send button (disabled = faded gradient). The emoji popover is a 9-column grid with a card shadow.
12. **P2 — Search bar:** the input plus a "brief ×" chip that echoes the query you just typed plus "1 of 1" plus ↑ ↓ ×. The tooltip "Search this conversation" is custom-drawn and clips at the right edge.
13. **P2 — Folder-activity rows** ("Alex added the folder Moodboard (3 items) to Project (shared with Alex) · 9.4 MB") wrap to two lines, with a bordered file row under each. They compete with messages.
14. **P2 — Empty thread:** "Say hi to Sam 👋" with an icon tile. The "no one to chat with" empty state is a card with an icon tile and a gradient button.
15. **P2 — Staged attachments:** chips with a paperclip icon and an × that reads as a chip-in-chip. They sit above the composer with no thumbnails.
16. **P3 — The long name** (Maximilian) truncates in the list, but the header then shows the full name on one line. The typing indicator is the plain word "typing…".

## 5. Locations (+ file browser)

1. **P1 — Synced-folder tiles** carry four labelled buttons (Sync now, Pause, Open folder, Remove), which wrap to two lines at 980px.
2. **P1 — A "Refresh" button** in the header (the page already refreshes every 30s and on events).
3. **P1 — Raw absolute paths** as primary text (`/Users/you/Pictures/Travel`, `/Volumes/buddy/Shared`).
4. **P1 — A dashed promo card** "Keep a folder on this device copied to one of these — no dragging, no thinking about it." with its own "Sync a folder here" button. It duplicates "+ Sync a folder" above it.
5. **P2 — Grid of large tiles** (each with a 48px icon tile, a pill badge, a title, a subtitle, a line of rights, a → arrow). Two columns of tall cards for what is a list of 1–5 items.
6. **P2 — Section headers** carry an icon plus a title plus a one-sentence explainer each ("Synced to a location", "Shared from this device").
7. **P2 — "Gateway" pill** and "Private · nobody has access yet", "No friend activity recorded yet", and member chips inside the gateway tiles.
8. **P2 — Page width differs** from every other page (`page-wide` 1100px, left edge 40px further left).
9. **P2 — "1 device(s) unavailable or without Locations support"** is a `<details>` with raw error strings.
10. **P2 — File browser:** a "All locations" ghost back button plus a host label row plus "Send & Receive →" (unrelated navigation) plus a toolbar of 7 labelled buttons.
11. **P3 — The empty state** is a large card with a 46px thin-stroke icon, a title and a two-line paragraph containing a path ("Settings → Locations").

## 6. Shared Folders

1. **P1 — Inverted hierarchy:** each card's title is the *peer* name ("Alex") and the folder name is the grey subtitle.
2. **P1 — Member pills** hold avatar, name, an Editor|Viewer segmented control and ×. With "Ashton's MacBook Pro (you)" as the first pill, this is a pill-in-pill-in-card.
3. **P1 — Every card has a mode pill** (Total sync / Two-way / View only (you receive)) and sometimes a "Paused" pill.
4. **P1 — Queued files** each render a *progress bar at 0%* with a stub fill, under a clock icon and "Queued". Three rows of empty bars.
5. **P2 — Header actions** are icon-only (open folder / pause / settings) with the pause glyph drawn as two columns (reads as a "columns" icon). Settings expands a drawer with toggles and four more buttons (History, Verify, Show invite, Unpair).
6. **P2 — Status dot** with a 3px glow ring; "Sync paused — Resume to merge changes" is jargon.
7. **P2 — Banners inside cards** (amber "View only: changes you make here are not sent", red "no longer shares this folder").
8. **P2 — The progress row** repeats the pattern: "62%" in gradient, a locality pill, "77.0 MB / 124.0 MB", "Stop", then the filename and "41.0 MB/s · 1s left" on a separate row.
9. **P2 — The pending invite** ("Pending peer", "Waiting to join…") shows a "?" avatar.
10. **P2 — The create dialog** has three large option cards with 3-line descriptions and a gradient full-width "Create & get invite".
11. **P3 — The empty state** paragraph explains mirroring in 3 lines.

## 7. History

1. **P1 — Raw IP instead of a name** ("Received from 192.168.1.40:5", "Sent to 70.2.1.9:5").
2. **P1 — A green check next to "Saved, unverified"** (contradictory). Every row carries the "Saved, unverified" disclosure line.
3. **P1 — A failed item** shows a red ✕ with no reason and no retry affordance.
4. **P1 — The two-tab segmented control** is full-width and 36px tall (brief #4).
5. **P2 — Rows** show a file-type icon tile plus a tiny direction badge overlapping its corner plus a locality pill plus status icons plus a reveal button, and a hover × to remove.
6. **P2 — Each date group** is a separate card; the search field is full-width with its own border.
7. **P2 — Canceled** entries render with no status at all.
8. **P2 — Recoverable:** the storage gauge is a card with a big "792.5 MB" and a blue bar. "Free up space" is a gradient primary (a destructive action as the primary). The per-folder sections are cards with a grey header band. Every item has a "Restore" outlined button plus a trash.
9. **P3 — "Clear list"** is a labelled outlined button with a trash icon in the page header.

## 8. Settings

1. **P1 — One 6,000px scroll** with ~40 rows, most with 2–4 line explanations ("Cap how much of your upload a transfer uses, so video calls, streaming, and browsing stay smooth — and so a big transfer doesn't overwhelm an older Wi-Fi router. 0 = unlimited…").
2. **P1 — The "How transfers connect" section** is a glossary of pill badges (Local network / Direct / Relay / Connecting) with paragraphs.
3. **P1 — "Direct peer-to-peer: On"** is a static green pill that looks like a toggle but isn't one.
4. **P2 — The Devices and Locations panels** are embedded with a different header style (an icon plus a title plus a subtitle, their own cards and their own buttons, including a red "Remove this Mac from account" next to the primary).
5. **P2 — Segmented controls** for "Keep copies for" and "Storage limit" are full-width, 4-up, 36px tall.
6. **P2 — The upload limit** combines a numeric input ("0 Mbps") plus a preset pill row ("100 ★").
7. **P2 — The relay URL row** puts "Restart" next to an empty input, with a 6-line description that includes a GitHub path.
8. **P2 — Diagnostics** puts "Restart" next to a toggle, and the endpoint input is labelled "Built-in (leave blank)".
9. **P2 — "Recent activity"** under Locations shows raw operations ("upload: Taxes/2026/W-2.pdf") with a full timestamp.
10. **P3 — The eyebrow section titles** (uppercase, letter-spaced) differ from the panel titles above them.

## 9. Dialogs & sheets

1. **P1 — Every dialog header** has a 38px accent-tinted icon tile, an 18px/750 title and a subtitle. Full-width gradient primaries appear throughout.
2. **P2 — Dialog panels** use a 20px radius, a 4px backdrop blur and a spring scale-in.
3. **P2 — Send to…:** 40px gradient avatars with overlapping presence and device badges, and "Offline — a send keeps trying for ~2 minutes" repeated on every offline row. The "Share with a code or QR" row sits in a separate block.
4. **P2 — Link a device** has no close ×, a numbered paragraph, and three secondary buttons stacked on the right ("Scan the other device's code instead", "Copy code", "Cancel").
5. **P2 — QR scanner:** a black camera well, "Starting camera…", two outlined buttons and a keyboard-shortcut paragraph (`⇧⌘^4`).
6. **P2 — Block / Report** are long paragraphs, and there are three buttons on the footer's two sides ("Report instead…", "Cancel", pink "Block").
7. **P2 — The incoming folder invite** has a 2-line explainer and a gradient "Accept & choose folder".
8. **P2 — Sync a folder sheet:** a numbered stepper with circled digits, plus a paragraph, plus an inline primary, plus a separate "Cancel" in the footer.

## 10. Menu-bar popover

1. **P1 — The header** shows a settings cog, "DropBeam" and a power icon (Quit) next to ×. The Quit sits one pixel from Close — dangerous.
2. **P2 — The search field** is a large rounded pill with an accent border.
3. **P2 — Friend rows** are 36px gradient avatars with "Tap or drop a file" (a mobile-ism, "Tap") under offline friends.
4. **P2 — The footer** has a gradient "Send a file" primary with glow, plus a bordered square receive icon button with no label.
5. **P3 — Recent transfers** (when present) squeeze below the list.

## 11. HUD (folder-sync pill)

1. **P2 — The gradient app-icon tile** plus a title plus a sub-line, and the percentage repeated twice ("9 of 12 files · 72%" and a big "72%"). A locality icon chip sits beside them.
2. **P3 — The × close button** is a filled grey circle.

## 12. Floating receive card

1. **P1 — Layout collisions** at 190×184: the filename overlaps the avatar ring and the "From Alex" line clips at the bottom edge.
2. **P2 — Gradient accept button** with a split caret; the Decline button is a dark pill with the same weight.
3. **P2 — The progress line** "53% · 12.0 MB/s · 3s left" is 3 facts at 11px.

## 13. States & edge cases

1. **P2 — Very long file names** wrap in some cards (History, Locations tiles), truncate in others and ellipsise mid-word in the receive card ("Voice Over M… (Fixed).txt"). One truncation rule is needed.
2. **P2 — Offline friends** read differently on each surface: "Last seen 3h ago", "Tap or drop a file", "Offline — a send keeps trying for ~2 minutes", "Not seen yet".
3. **P2 — Relay transfers** show amber pills, which are warning-coloured for a normal, working state.
4. **P2 — Scrollbars:** the main pane uses a custom 10px thumb on macOS where the overlay scrollbar is expected.
5. **P3 — Numbers** that change (percent, bytes, speed) aren't tabular everywhere, so text jitters as they tick.
