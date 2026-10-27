# Embedded browser occlusion

Session browser pages belong to the executing Desktop and carry an immutable
conversation ID. The host creates Agent-requested pages hidden, registers them,
then emits the owning session and exact native target ID. Page readiness never
depends on an active conversation, a mounted panel, or visible viewport bounds.
Tool discovery and attachment filter by conversation, including explicit page IDs;
legacy callers without a conversation retain their unscoped contract.
The shared new-window link interceptor runs at native document initialization, so
background navigation has the same behavior before a panel has ever mounted.
Replacement navigation gates tool evaluation on the next native load lifecycle,
so a slow response cannot expose the previous completed document as the new page.
Fragment-only transitions keep their existing document and do not await a load.

On macOS, native main-frame navigation failures are retained and sent to both
browser tools and the mounted panel. Failed navigation ends pending reads with
the native reason instead of reporting that it is still loading. A subsequent
manual navigation replaces that outcome; no automatic reload is added. A page
whose native URL is temporarily unavailable can still be positioned and shown,
and its address updates when navigation commits. Attachment after an earlier
failure shows the retained failure without hiding or recreating the native page.

The frontend commits each open event directly to that session's canvas snapshot
without activating the session or touching another session's selected tab. Browser
snapshots survive the ordinary canvas LRU eviction limit. Manually opened session
tabs also receive a stable target ID and conversation owner.

Both `BrowserPanel` and `BrowserScene` use `useEmbeddedBrowserWebview`. Session
panels attach to the retained native target, read its current URL, and only hide
it on unmount; switching conversations never reloads the initial URL or closes
the document. Closing presentation leaves an Agent's page available until an
explicit browser close or session cleanup. Standalone, unowned browser surfaces
retain their component-owned lifetime. Native presentation calls share a per-target
queue across remounts so stale cleanup cannot hide the new presentation. WebKit's
background suspension is disabled for session-owned pages on supported systems.

All show/hide/focus paths go through `nativeWebviewVisibility`.
DOM floating surfaces declare `data-openbitfun-native-webview-occlusion`; their
overlapping bounds suppress native visibility without destroying the page.
Visible browser surfaces share one DOM observer. It tracks inserted/removed
overlays and their relevant ancestors rather than rescanning the document for
unrelated class/style mutations. Hidden tabs do not subscribe; document hiding
pauses observation. Relevant popup mutations still reconcile before the next paint.

`isLoading` follows native `browser-webview-page-load` events and the child
document's readiness rather than IPC completion. Subscribe before creating the
view so fast pages cannot finish before the listeners are ready. After a native
commit or completion, `browserPageLifecycleScript` checks `document.readyState`
and observes `pageshow`, `popstate`, and related load events. This settles history
restoration even when a matching native completion notification is absent or late.
The bridge uses the existing event permission and accepts only the current view
label and loading sequence; cached pages and delayed scripts cannot finish a newer
navigation. Do not probe before commit, when the previous document can still be
complete. A 30-second watchdog remains a last resort if both signal paths fail.

Both surfaces show the loading state as an address-field spinner and turn the
refresh action into a stop action that runs `window.stop()` in the child view.
Failed stop requests retain the stop action and display the error so users can retry.

`BrowserPreviewCache` retains one decoded JPEG in memory beneath the native view.
It samples only while the native page and document are visible, waits at least five
seconds between completed captures, and allows at most one request in flight.
Identical frames skip decoding and publication, and increase the sampling interval
to 10, 20, then 30 seconds. A changed frame restores the five-second interval.
Rapid popup/tab toggles reuse a recent frame. Hiding never
waits for a screenshot. The resulting placeholder is decorative and cannot receive
input. Navigation, native viewport size changes and target replacement invalidate
the frame and reject stale responses. Frames are not persisted.

Native viewport edges are aligned to physical pixels using the controller's device
pixel ratio. The preview uses that same applied rectangle relative to its DOM host,
with square corners and explicit dimensions; generic image styles must not alter
its geometry. A one-physical-pixel change must reach the native view even at fractional DPI.

The desktop `browser_webview_capture_preview` command captures the specific child
view, bounds the longest edge to 1600 pixels, and encodes JPEG at quality 80 off
the async runtime worker. Windows and macOS use the existing WebView screenshot
adapter. Linux currently returns an explicit unsupported response. Unsupported or
older hosts retain the normal placeholder; transient capture failures back off
up to 30 seconds and keep the last valid frame. This decorative placeholder may
lag live content; it is not a live video mirror or a memory-leak diagnostic.

The command is controller-local in Peer Device Mode and never requests a screenshot
from the peer. SSH/Docker workspaces do not change ownership of this local UI view.
The web renderer keeps its existing iframe path. This command adds no preview
transport to mobile/IM remote control or detached jobs; peer hosts refuse it.
Current-URL reads are controller-local too. Peer browser-open events never attach
their labels to local native pages. Agent browser tools retain the explicit remote
workspace gate, and CLI/headless dispatch hosts still report an unavailable native
browser; this lifetime change adds no remote browser transport.

Focused verification:

```sh
pnpm --dir src/web-ui run test:run src/app/scenes/browser src/infrastructure/api/adapters/peer-device-adapter.test.ts src/flow_chat/services/flow-chat-manager/EventHandlerModule.test.ts src/app/scenes/session/sessionPaneLayoutStore.test.ts
```

Manual checks: after rebuilding the desktop host, open a browser and let it settle,
then open session overview or a tab context menu over it. The last page frame should
remain in the uncovered region; closing the popup should restore the live page.
Also check navigation, resizing, multiple browser tabs and a video page. The frozen
image is not evidence that media playback or page execution has paused.

For native multi-conversation acceptance after building Desktop and Web UI:

```sh
node tests/e2e/scripts/run-browser-session-isolation.mjs
```

This isolated runner scripts only the model HTTP response. Real Agent tool calls,
native pages and production session controls exercise background opening, delayed
replacement, concurrent reads, repeated session switches, retained document/input
state, and native visibility.
It does not establish remote-host or cross-platform behavior.
