# Future plugin marketplace

**Status: design plan only.** PhotoCraft currently installs independent local packages and
supports optional per-plugin GitHub release updates. No marketplace service, account flow,
listing browser or publisher verification service is implemented.

The marketplace should help people discover compatible plugins while preserving direct
installation, independent releases and offline use. A listing points to a plugin's own
release artifacts. Installing a plugin never requires another plugin, a source checkout,
a companion application or a shared marketplace runtime.

## Boundaries

The catalog owns discovery metadata, search, moderation and distribution trust. The plugin
owns its source, algorithms, dependencies, licenses, settings and release schedule. The
existing engine owns package validation, permission review, installation, enable/disable,
managed data, cancellation and rollback. Catalog installs and updates must use those same
engine commands; a marketplace must not introduce a second installer or bypass native-code
consent.

Listings should declare stable plugin/publisher identity, descriptions, license/source
links, screenshots, release versions, supported platforms, required host capabilities,
execution profile, permissions, package size and integrity information. Accepted platform
metadata must remain distinguishable from actual conformance results. Search and downloads
must not execute plugin code.

## Delivery sequence

1. **Catalog contract and read-only discovery.** Define a bounded versioned catalog schema
   with deterministic pagination, compatibility filters and signed publisher/release
   references. Prototype search and plugin detail views using neutral entries. Keep manual
   installation usable when the catalog is offline.
2. **Publisher and release trust.** Define signing identity, ownership verification, canonical
   integrity indexes, key rotation, repository transfers, revocation and retained metadata
   for reproducible releases. Bind each asset's plugin ID, release version, target, size and
   digest to the verified publisher. Keep unsigned development installation explicit.
3. **Installation integration.** Resolve a catalog release into a reviewed candidate, prepare
   it through the existing update/package pipeline, show permission changes, then publish
   through the managed store. Move large archive extraction and managed-data preparation
   into background staging and keep the guarded publication section short. Test interrupted downloads, changed metadata, stale reviews,
   full disks, competing editor instances, migrations and rollback.
4. **Publishing and operations.** Add publisher submissions, clear review rules, reporting,
   license/provenance checks, malware response and revocation notices. Define service
   retention, privacy, rate limits, recovery and availability responsibilities before a
   public launch. Accounts or paid listings require a separate product decision.

Do not reserve a visible marketplace tab or nonfunctional action before discovery exists.
When implemented, the manager can add a browse surface while retaining its compact installed
list and update controls. A catalog outage must leave installed plugins functional and
manual/offline packages usable.

## Release gates

Verify independent plugin builds and runtime ownership, compatible target selection,
bounded hostile catalog handling, signature/key-rotation behavior, permission expansion,
revocation, cancellation and paired code/data rollback. Certify actual macOS/Linux/Windows
behavior for every execution profile advertised; a separate native process is not proof of
OS containment. Publish measured search/download and large-registry results before claiming
scale qualification.

This work belongs to the distribution and trust phase of the
[plugin platform plan](plugin-platform-plan.md#74-distribution-and-publisher-trust).
The [current update contract](plugin-updates.md) is the initial distribution mechanism;
marketplace discovery should extend it without creating parallel APIs or mandatory services.
