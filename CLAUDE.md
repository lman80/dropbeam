<!-- superfeedback:handling v1 -->
## User feedback (SuperFeedback) — keep the Issues up to date

Users send bugs and ideas from inside the app. Each one becomes a GitHub Issue in
`lman80/dropbeam` with the `feedback` label plus a type label (`bug`, `feature`, `other`, `crash`).
The body has the user's message, a screenshot link, a context table (device, OS, app version…)
and often recent logs. **An open Issue counts as "not addressed" on the owner's dashboard**, so
keep them accurate as you work:

1. **At the start of a session**, look at what's waiting:
   `gh issue list -R lman80/dropbeam --label feedback --state open`
   Read the ones related to your task (`gh issue view N -R lman80/dropbeam`). If the user's request
   is about an area you're changing, fix it too or mention it to the owner.
2. **When you start on one:** `gh issue edit N -R lman80/dropbeam --add-label building`
3. **When it's fixed:** put `Fixes #N` in the commit message (GitHub closes it when the commit
   reaches the default branch). If the fix lands any other way, close it yourself:
   `gh issue close N -R lman80/dropbeam --reason completed --comment "Fixed in <commit or version>: <one line>"`
   If the Issue also has the `idea` label: `gh issue edit N -R lman80/dropbeam --add-label shipped`
   and set its milestone to the version it ships in (users see that on the public roadmap).
4. **Already fixed earlier** (you find the code already does it): close it the same way and cite
   the commit or file that fixed it.
5. **Won't fix, can't reproduce, or duplicate:**
   `gh issue close N -R lman80/dropbeam --reason "not planned" --comment "<why, or Duplicate of #M>"`
   Ask the owner before declining a feature request — don't decide product direction yourself.
6. **Before you finish:** every Issue you fixed this session is closed with a comment, and every
   one you started but didn't finish still has `building` and a comment saying where it stands.

Never close an Issue without a comment, never delete Issues, and don't edit the report body
(it holds the context table and, for public ideas, the vote-count line).
<!-- /superfeedback:handling -->
