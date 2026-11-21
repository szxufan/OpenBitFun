[中文](AGENTS-CN.md) | **English**

# GenUI history viewer

FlowChat replays recorded GenUI tool results and native source streams from older hosts here. Load the OpenUI interpreter on demand for historical tool results. New answers use the built-in visualize skill and the viewer in `src/web-ui/src/tools/visualize`. Keep durable architecture facts in [the existing architecture document](../../../../../docs/architecture/generative-ui-architecture.md).

- Keep recorded identities, library revisions, source, and initial state intact. Direct and deferred calls use the producing session and surface. Preserve unknown formats and expose the saved content when the renderer is unavailable.
- Historical views are read-only. Opening a message must not start a repair turn, fetch a host provider, persist an edit, or execute document scripts.
- Use OpenUI lang-core and react-lang for recorded component programs. The pinned library owns schemas and trusted renderers. Read native GenUIStream events with their sequence and reset markers. Keep these separate from tool-argument fragments.
- Render HTML/CSS/SVG documents in the opaque-origin sandbox with structured sanitization. Keep the historical host disconnected from script bridges, native APIs, credentials, filesystem and network. Preserve the sandbox constraints of the shared static viewer.
- Keep vendoring and license information in components/vendor/README.md.
- Preserve hidden-child and nested-stack spacing in global selectors. Verify hot selector changes with native style invalidation evidence and traces.

## Verification

Run the narrow checks for the area you changed.

    node scripts/generate-genui-contract.mjs --check
    pnpm --dir src/web-ui exec node scripts/run-vitest.mjs run src/tools/generative-ui src/tools/generative-widget/GenerativeWidgetPanel.test.tsx src/flow_chat/utils/generativeWidgetProjection.test.ts --maxWorkers=2
    pnpm run check:web

For stored settings, verify the upgrade round trip at the configuration owner.

    cargo test --locked -p openbitfun-config-contracts --lib generative_ui_library_legacy_and_future_roundtrip

Start Vite in this checkout. The two browser runners use an installed Chrome or CHROME_PATH. They exercise local rendering and replay with recorded inputs. Use real hosts to verify remote transport behavior.

    OPENBITFUN_DEV_PORT=1430 OPENBITFUN_DEV_HMR_PORT=1429 pnpm --dir src/web-ui exec vite --host 127.0.0.1
    OPENBITFUN_GENUI_TEST_URL=http://127.0.0.1:1430/tests/e2e/generative-ui.html node scripts/test-genui-browser.mjs
    OPENBITFUN_VISUALIZE_TEST_URL=http://127.0.0.1:1430/tests/e2e/visualize.html node scripts/test-visualize-browser.mjs
