# PhotoCraft plugin platform changelog

This log covers the shared plugin integration in [pull request #6479](https://github.com/storytold/photocraft/pull/6479).
Dates identify integration changes, not PhotoCraft application releases. Each independent
plugin keeps its release history in its own repository.

## 2026-10-16

### Added

- A standalone typed API and guest SDK, one authoring manifest, generated descriptors and
  bindings, private SDK source exports, and developer commands for creating, checking,
  building and packaging independent projects.
- Thirteen public JSON Schema snapshots, five standalone starters, developer documentation
  and agent guidance.
- Lazy discovery from cached descriptors and background verification of package payloads.
  Opening the manager does not start native workers or load model payloads.
- Private persistent native workers through explicit worker protocol 2, with framed messages,
  cancellation, deadlines, reset, retry, idle eviction and quarantine.
- Fair native-job scheduling, bounded queues and preview retention, and resource admission
  estimates. Each plugin retains its own process and runtime state.
- Managed settings, data, cache and secret references, declarative data migrations, and
  rollback of the previous code/data pair. Removal preserves managed data unless purge is
  requested.
- Explicit GitHub release checks and verified update downloads for plugins that declare a
  repository and exact release asset. The manager shows available versions and reviews
  permissions before installation.
- Optional automatic desktop release checks after startup and daily, with a persisted opt-out
  and attempt timestamp, quiet background failures, and an available-update count in the Plugins
  menu. Explicit installation review remains required; Update refreshes expired discovery authority.
- A [marketplace delivery plan](plugin-marketplace.md) covering discovery, publisher trust,
  signatures, revocation and reuse of the existing installer.

### Changed

- Maintain one PhotoCraft Plugin API with internal wire discriminator `1`.
  Package format 2, filter ABI 1 and native worker protocols 1/2 remain separate contracts.
- Browse now opens permission review. Install or Update confirms the action without a
  replacement checkbox. Same-version reinstall/repair and deliberate downgrade/replacement
  sit under Advanced.
- Removed the separate drop zone; the manager accepts a file dropped into its window.
- Clear the selected file after a successful installation and show the plugin name and
  completed action. Keep the selection when installation fails.
- Keep feature algorithms, model catalogs, assets, binaries and their notices in independent
  plugin repositories. Core compatibility fixtures use neutral examples.

### Fixed

- Keep failed desktop installations visible with safe Remove and compatible Update/Repair
  actions. Bind recovery to the discovered record/activation, preserve managed data, and
  retain unverified corrupt payloads rather than deleting files without ownership evidence.
- Complete the automatic-update preference translations and hide the internal cooldown
  timestamp from the Preferences dialog.
- Preserve saved export presets when loading preferences alongside the upstream recovery
  of individual damaged preset sets.
- Release store and generation locks when their owning guard ends, including when a Unix
  subprocess temporarily inherits a duplicate file descriptor.
- Preserve the Cargo launcher name when building independent SDK examples through rustup.
- Reject stale installation reviews and update candidates after the selected package,
  installation, store or activation changes.
- Protect files used by active and idle workers with per-generation leases. Recheck authority
  before accepting results or applying document edits.
- Preserve the rollback choice across failed or interrupted publication.
- Keep lifecycle and update administration in the generated command reference.

### Compatibility and limits

GitHub updates require a public stable release and a matching asset digest. Final archive
installation remains synchronous. Native resource declarations are admission estimates;
OS containment, native macOS/Linux qualification and release-scale performance qualification
remain open. Marketplace discovery is a plan. See [updates](plugin-updates.md),
[lifecycle](plugin-lifecycle.md) and the [platform plan](plugin-platform-plan.md).

## 2026-10-15

### Added

- Installable import-free WASM panels, declared command permissions, and enable/disable/remove
  integration through shared engine commands.
- Package-owned native workers and WASM filters using the general image-processing boundary,
  progress, cancellation, comparison previews, Apply/Discard and Undo.
- Contributed commands, bounded layer queries, atomic rename/visibility edits, document
  invalidations and read-only inspectors.
- Neutral source starters, binary compatibility fixtures and plugin qualification checks.

### Fixed

- Validate stored ZIP payload sizes before allocation when reading plugin packages.

## Maintaining this log

Add dated entries for completed platform changes, including compatibility changes and actual
limits. Keep planned work in the platform plan. Record plugin-specific releases in each
plugin's own `CHANGELOG.md`.
