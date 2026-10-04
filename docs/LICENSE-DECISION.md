# Choosing DropBeam's licence (owner decision)

Right now the repository is public but has **no licence file**. Legally that means "all rights reserved": people can read the code but can't reuse, modify or redistribute it. The app's own bundles are covered (they ship as the owner's software), and third-party code is credited in `THIRD-PARTY-NOTICES.md`. This page sets out the options so you can pick one. Nothing has been chosen.

Context: the plan is **"free app, paid hosted services"**, meaning the desktop and iOS apps are free and money comes from hosted extras (a managed Transfer Server / relay, push, maybe storage).

## Dependency constraints

All 949 third-party components are permissive (MIT, Apache-2.0, BSD, ISC, Zlib, Unicode, CC0, BSL-1.0) or weak copyleft limited to their own files (MPL-2.0: `cssparser`, `selectors`, `attohttpc`, `option-ext`, `dtoa-short`, `lightningcss`). None of them forces a particular licence on DropBeam, so every option below is allowed. MPL-2.0 only requires that changes to *those files* stay MPL, and we haven't changed them.

## Options

| | MIT / Apache-2.0 | AGPL-3.0 | Source-available (e.g. FSL / BSL / PolyForm) | No licence (status quo) |
|---|---|---|---|---|
| Others can fork & ship their own DropBeam | Yes, even closed-source | Yes, but it must stay AGPL, including hosted versions | Not for competing use; becomes open after 2–4 years (FSL/BSL) | No |
| A competitor can sell **hosted** DropBeam services | Yes | Yes, but they must publish their server-side changes | No, during the protected period | No |
| App Store (iOS) | Fine | Awkward: the App Store terms conflict with GPL-family licences. You can still publish your own build as the copyright holder, but contributors' code complicates that. | Fine | Fine |
| Outside contributions | Easiest | Need a CLA if you might ever relicense or sell commercial licences | Need a CLA | Basically none |
| Trust / "can I audit the crypto?" | Highest | High | Medium (code is visible) | Medium (visible, no rights) |
| Fit with "free app, paid hosting" | Good for adoption. Protects the hosted business only through brand and convenience. | Strong protection for the hosted business; may scare some users and companies | Strongest protection; less "open source" credibility | Protects everything, but invites no community |

### Apache-2.0 vs MIT
If you go permissive, **Apache-2.0** is the better choice. It adds an explicit patent grant and a NOTICE convention, and it matches iroh/noq. You could also dual-license MIT OR Apache-2.0 the way the Rust ecosystem does.

### Trademark
Whatever the licence, keep the **"DropBeam" name and icon** as trademarks (state it in the README). Then forks have to rename, and your hosted service stays the "real" one. This matters most with a permissive licence.

## Recommendation

**Apache-2.0 for the apps, plus a trademark notice, with server-side hosting code kept in a private repo** (or source-available under FSL-1.1-Apache-2.0 if you want it visible).

Reasons:
1. The apps are free anyway. An open, auditable client builds the trust a P2P encryption app needs, and it doesn't conflict with the App Store.
2. The paid product is the hosted service (operations, uptime, push keys, storage). A licence protects it less than simply not publishing the hosting/billing code, plus the brand.
3. Apache-2.0 matches the core dependencies (iroh, noq), so vendored patches and upstream contributions stay simple.

If keeping competitors from selling hosted DropBeam on top of your code matters more than adoption, pick **FSL-1.1-Apache-2.0** for the whole repo. It becomes Apache-2.0 two years after each release. Choose AGPL-3.0 only if you're happy to require a CLA and work around the App Store.

## What to do once you choose

1. Add `LICENSE` at the repo root (the full text; for Apache-2.0 also a short `NOTICE`).
2. Set `license = "<SPDX id>"` in `src-tauri/Cargo.toml` and `"license"` in `package.json`. Add `"license": "<SPDX id>"` to `bundle` in `src-tauri/tauri.conf.json`, so the .deb/.rpm/MSI metadata matches.
3. Replace the "Licence" section of `README.md`, and add the trademark sentence.
4. If you might want to relicense later, add a CLA (for example the CLA Assistant GitHub app) before accepting outside pull requests.
