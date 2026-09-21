# Mobile kit

Import components and types from `src/mobile/kit`. `mobile.css` imports `kit.css`;
all selectors require `html.mobile`. Do not mount this kit on the desktop.
No component imports the store, Tauri, or application business logic.

- `Screen({title, leading?, trailing?, children})` owns scrolling and the safe-area
  top. `leading` accepts `{title, onPress}` for a back button or any React node.
  Its large title scrolls into a 44pt `NavBar`; do not nest another scrolling page.
  `NavBar` is also exported for custom conversation/browser chrome.
- `Section({title?, footer?, children})` owns 16pt inset and grouped `List` cells.
  `Row` takes `title`, `subtitle`, `value`, `icon` OR `avatar`, `onPress`,
  `accessory` (`chevron`, `toggle`, `checkmark`, `none`), `checked`, `onChange`,
  `disabled`, `destructive`, `tint`, `emphasized`, `trailing`, and `data-testid`.
  `icon={<IconSquare color="…"><Glyph/></IconSquare>}` supplies a Settings icon.
  Trailing controls are siblings of the row button, never nested buttons.
- `Button` is a text/bar button; `filled` is a 50pt sheet CTA. `Switch` takes
  `checked`, `onChange`, `label`, `disabled`. It has a 51×31 visual in a 44pt target.
- `TextField` requires a `label` and accepts normal input props. Put it in a
  Section. `SearchField` takes `value` and `onChange(string)`. `SegmentedControl`
  takes `label`, `options: {value,label}[]`, `value`, and `onChange(value)`.
- `Avatar({name, src?, size?, badge?})` is circular with initials and a 16pt device
  badge. Convert native file paths to URLs in the caller. `Badge({count})` hides
  zero. `EmptyState({icon,title,body?})` deliberately has no action.
- `ProgressRow` adds `progress: number | null` (0–100) to Row; null hides the track.
- Mount `Sheet({title,onClose,primary?,size?,dismissible?,children})` conditionally.
  Size is `medium` or `large`. Its content accepts Sections. `primary` is normally
  a Button labeled Done. A non-dismissible sheet has no Cancel/backdrop/Escape
  dismissal. Sheet title and actions stay fixed while content scrolls.
- `ActionSheet({title?,message?,actions,onClose,dismissOnAction?})` has a separate
  Cancel group. Actions are `{label,icon?,destructive?,disabled?,onPress}`.
  By default it calls `onClose` before `onPress`; set `dismissOnAction={false}`
  only when an action itself resolves/dismisses a picker.
- `Alert({title,message?,children?,actions,onClose})` supports a TextField child.
  Alert actions close explicitly, allowing validation/persistence before closing.
- `ContextMenu({children,actions,label?})` opens on a 400ms touch hold, mouse click,
  Enter/Space, or context-menu gesture. Pointer movement cancels the hold.
  The trigger should contain presentational content, not another button/link.

Overlays portal native modal dialogs to body: focus is trapped, background is
inert, Escape/backdrop dismisses, and nested dialogs share a scroll lock. Callers
own async work, errors and disabled states. All actionable targets are at least
44pt (search's 36pt field is the explicit platform-style exception). Only bars
blur. System colors, `--accent`, and `--ui-font-scale` are the public theme inputs.

`MobileApp` owns a small local push stack; App's keyed content ErrorBoundary resets
it on top-level tab changes. Native `native-tab` events still use the existing
store listener. App reserves `--mk-bottom` once: native safe-area bottom already
includes UITabBar; browser preview adds 49pt; keyboard-visible removes it. Screens
must not add another tab-bar inset. Sheet overlays use their own safe-area bottom.
Legacy Chat/History/Locations use `.mk-legacy` until their slices migrate to Screen.
