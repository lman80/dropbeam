# iOS UI audit — native SwiftUI shell (2026-09-25)

Scope: every screen of the SwiftUI shell in `src-tauri/plugins/native-ui/ios`, audited in the
iOS 26.5 simulator (iPhone 17; a Pro Max simulator was booted but input to it needed a
permission nobody was present to grant, so wide/large layouts were covered with Dynamic Type
Accessibility M/L instead), light and dark, with real data (paired with a Mac, text/photo/video/file messages,
Quick Send, reactions). Status: **F** = fixed on branch `ui-ios`, **D** = deferred (reason given).

## Global

1. **F** Violet mesh-gradient glow drifting behind every screen (`BeamBackground`) — the main
   "AI" tell. Screens now sit on the plain system grouped background like Settings/Files.
2. **F** Initials avatars use a violet→blue gradient. Now the Contacts/Messages grey monogram.
3. **F** Toggles are tinted brand violet everywhere; system green is the iOS convention and keeps
   the accent for actions only.
4. **F** Brand violet in dark mode is too light for white text on filled buttons (≈3.4:1).
   Deepened slightly (≈4.6:1) without changing hue.
5. **F** Codes (friend code, Quick Send code, device-link code, invites) render as 3-line
   monospaced text with automatic **hyphenation** ("…Nz-" / "Qz-") — looks broken and would be
   copied wrong by hand. Codes are now one middle-truncated line (Copy/Share carry the full value);
   the paste field is single-line.
6. **F** QR codes sit in a white plate with a drop shadow inside an already-white card.
   Shadow removed; the white plate only remains where needed for contrast.
7. **F** `MediaThumbnail` and `FriendAvatar` reset their image to nil on every size/identity
   change → avatars/thumbnails flash to initials/placeholder when rows re-layout (keyboard,
   scrolling, rotation). Images are now kept until the replacement is ready, and a synchronous
   cache hit renders on the first frame.
8. **F** Liquid-glass "button soup": three glass capsules inside a `GlassEffectContainer` that
   morph into each other (Send/Message/Check on friend page, Pause/Open/Verify on folders,
   Photos/Files/Folder on Send render as ovals). Replaced with one reusable Contacts-style
   `ActionTile` (rounded-rect, tinted icon + caption, equal widths).
9. **F** Explanatory footers under nearly every section; many repeat what the row says. Trimmed
   to the ones that carry real information.
10. **F** Offline banner is a two-line sentence in a capsule; now "You're offline" + short hint.

## Send

11. **F** Hero "Send something good." + tagline + decorative plane duplicate the large title.
    Removed; the three source tiles lead the screen.
12. **F** Photos/Files/Folder buttons are glass capsules 88 pt tall → render as circles/ovals with
    the label touching the edge; Photos alone is filled. Now equal rounded-rect tiles.
13. **F** Empty "Nothing in Flight" illustration (dotted circle, sparkle, rotated plane) and cute
    copy ("make someone's day"). Now a quiet one-line empty row.
14. **F** "Have a Code?" footer is a 3-line paragraph. Shortened.
15. **F** Transfer rows are button soup: route pill ("Local"), "Verified end to end", an
    "Integrity details" disclosure and a "Verify Copy" button on *every* finished row. Now:
    title, who · state · size, one trailing action. Integrity/verify appear only when something
    needs attention (unverified files, a verify the user started); "Verify Copy" stays in the
    context menu.
16. **F** Progress line shows "Local · 18 ms" route pills (jargon + latency). Now plain speed/ETA,
    plus "via relay" only when it explains slowness.
17. **F** Waiting Quick Send subtitle "Share this code to send" wraps under the trailing buttons;
    shortened to "Waiting for a receiver".

## Friends

18. **F** Footer paragraph under Locations/Shared Folders/My Code. Removed.
19. **F** Empty friends card: "Good things are better shared." Now a plain empty state.
20. **F** Friend page action row: three merging glass blobs incl. **Check** which prints
    "Direct · 18 ms". Now Send / Message / Locations tiles; the connection check is a
    "Test Connection" row with a plain result ("Same network", "Direct connection",
    "Through a relay", "Not reachable").
21. **F** Avatar framed by a material ring + white stroke. Plain avatar like Contacts.
22. **F** When the page scrolls, the big name slides under the back button and the nav bar
    stays empty (title was hidden with an empty principal item). The name now fades into the
    nav bar once the header scrolls away (iOS 18+).
23. **F** "Rename" row with an orange pencil tile; now an "Edit" toolbar button like Contacts.
24. **F** Remove Friend / Report / Block split over two sections with icon labels; one plain
    destructive section now.
25. **F** Add Friend sheet: hint text missing in paste mode (simulator/no camera); a small Paste
    pill sits next to a large Continue. Hint restored; Paste is inline in the field (like the
    Send tab's code field) with one full-width Continue.

## Chat

26. **F** Thread header is off-centre (principal item centred between asymmetric toolbar
    groups) and the name turns white over blue bubbles when content scrolls beneath it.
    Name now sits in a glass capsule (iOS 26 Messages); search moved into the "…" menu so the
    header is centred.
27. **F** Sent file bubble shows a download arrow (↓) for a file that is already on this
    iPhone. Arrow removed; the clock only shows while a file is still arriving.
28. **F** Single photo/video bubble is always cropped square (16:9 clip becomes a square).
    Single media now uses its real aspect ratio (clamped like Messages).
29. **F** Staged document in the composer shows only a generic glyph with no name. Shows the
    file name.
30. **D** Received-message states could not be exercised live (the Mac didn't send); code reviewed.
30a. **F** After an app update/reinstall every photo/video bubble and history thumbnail turned
    into a grey placeholder: the stored absolute paths point into the *previous* app container
    (iOS moves `…/Data/Application/<UUID>/` on update). The UI now re-roots such paths into the
    current container when the old file is gone (`LocalPaths.resolve`).
30b. **F** Video tile without a thumbnail stacked the play badge on top of the film glyph;
    the badge now waits for the frame.
30c. **F** In-thread search footer said "No matches" before anything was typed.
30d. **D** Engine: reopening a thread a second time after launch shows only the first photo of a
    4-photo message + "Waiting for files…" (the restored transfer's share paths come from the
    web retry cache); and a Rust panic in `iroh::socket::remote_map::RemoteStateActor::run`
    (panic in cleanup → abort) crashed the app once while navigating back
    (~/Library/Logs/DiagnosticReports/DropBeam-2026-09-25-192743.ips). Outside the UI layer.

## History

31. **F** File names truncate at the end ("UI test notes with a very l…"); now middle-truncated
    so the extension stays visible (History and Send).
31a. **F** Row subtitle ends with the route ("· Local") which wraps to its own line as "· Local".
    Route removed.
32. **F** Peer shown raw — IP:port or an endpoint id can appear. Now the friend's name, or
    nothing when the peer is only an address.
33. **F** Toolbar "…" disappears when switching to Recoverable → title bar layout shift.
    The menu is always present and holds the actions for the current segment.
34. **D** Sent photos show a generic glyph instead of a thumbnail — history entries for sends
    carry no local path (engine data).

## Locations

35. **F** Toolbar refresh button (and a "Refresh" menu item in the folder browser). Refreshing
    is automatic + pull-to-refresh; the toolbar only shows a spinner while checking.
36. **D** Folder browser couldn't be exercised live (no Location shared with the simulator);
    reviewed in code.

## Settings

37. **F** Main Settings list is long and mixes power-user switches (Direct Connections Only,
    Wait for a Direct Link, Parallel Streams, Megabits, Upload Limit, Custom Relay) with everyday
    ones. Moved into one "Transfers" page.
38. **F** Profile: pencil icon after the name floats away from a wrapped name. Name is centred;
    editing is the toolbar "Edit" button.
39. **F** Profile/My Code "Share" button loses its icon while "Copy" keeps it; footer says
    "Friends → + → Add Friend". Consistent labels, plain footer.
39a. **F** Link-a-Device instructions tell a new phone to tap "Already use DropBeam?", but the
    onboarding button is "Link to Your Account".
40. **F** Devices page is a ScrollView of glass cards with 3 full-width buttons, unlike every
    other screen. Rebuilt as an inset-grouped list.

## Sheets & onboarding

41. **F** Onboarding icon is a violet→blue gradient tile with a coloured glow shadow. Now the
    real app icon.
42. **F** Send To: an empty "My Devices" section with an instruction sentence; very long names
    wrap to three lines. Empty section hidden; names limited to two lines.
42a. **F** Send → Photos/Files/Folder → pick → **the Send To sheet sometimes never appeared**
    (reproduced 3 of 4 times under load). The bridge re-pushes the web store's empty
    `pendingSend` snapshot right after every pick reply; when the picker finished dismissing
    first, that snapshot cleared the freshly picked paths before SwiftUI presented the sheet.
    Picks are now held in a Swift-owned `pickedToSend` queue the snapshot can't clear.
43. **D** The notification permission alert appears on top of onboarding at first launch
    (requested by the notification plugin at startup, outside the UI layer).
44. **D** The SuperFeedback floating button overlaps content and system sheets — owner's
    feedback tool, intentionally global; it can be switched off in Settings.
