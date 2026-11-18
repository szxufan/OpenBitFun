# OpenBitFun Playbook

OpenBitFun Playbook (<https://playbook.openbitfun.com>) is the bilingual (`zh-CN` / `en-US`) user handbook for OpenBitFun's features and settings.

It consumes `../docs/interactive-capabilities/capabilities.json`, generated from `../src/shared/interactive-capabilities/catalog.json` via `pnpm run capabilities:generate` — sharing the same semantic source of truth as in-app Global Search (`⌘K`) and the `OpenBitFunControl` agent tool while presenting warm, task-oriented guidance for human readers.

## Architecture & UX Highlights

- **Task-Oriented Functional Directory**: Organizes features and settings into practical workflow groups in pedagogical order, paired with a 4-step Quick Start path and real-world scenario guides on the home page.
- **Additive Playbook Schema Support**: Consumes optional `catalog.playbook` (`hero`, `gettingStarted`, `scenarios`, `directoryGroups`), per-capability guide metadata (`directoryGroupId`, `order`, `entryPathZh/En`, `shortcut`, `highlightsZh/En`, `scenariosZh/En`, `tipsZh/En`, `relatedIds`), and per-item `descriptionZh/En`, with built-in fallbacks when fields are omitted.
- **In-Page Quick Jump (`⌘K` / `Ctrl+K`)**: Opens an instant bilingual search modal from any page without losing reading context, and deep-links matched sub-capabilities (`?focus=<itemId>#item-<itemId>`).
- **Canonical Theme Governance**: Consumes `@openbitfun/design-tokens` and `@openbitfun/theme-openbitfun` (`System`, `Light`, `Dark`) with zero raw color literals or token fallbacks.

## Commands

```bash
pnpm run capabilities:generate
pnpm run website:dev
pnpm run website:test
pnpm run website:build
pnpm run theme:color-audit:website
```

## Immutable Release Deployment

Every build emits `dist/release.json` containing an immutable `releaseId` hashed from the catalog digest, website sources, design tokens, theme CSS, and brand assets. Deploy each `dist/` output into an immutable `releases/<releaseId>/` directory and atomically repoint the `current` symlink.
