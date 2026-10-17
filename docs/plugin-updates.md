# Plugin installation and updates

Each plugin owns its source, release versions, downloadable packages and optional update
repository. PhotoCraft uses the same managed installation transaction for local files and
downloaded updates. Plugins do not share an updater process or require a marketplace account.

## Install, update and repair

Open **Plugins → Manage Plugins…** and choose a plugin file. Choosing a file inspects its
embedded identity, permissions, capabilities and native payload before installation. Files
can also be dropped anywhere in the manager. For a typed path, select **Show permissions**.

A new identity offers **Install plugin**. A newer version of an installed identity offers **Update plugin**;
no replacement checkbox is needed. Review the proposed access before confirming. Updating
replaces that identity's managed installation and preserves its enable state. The selected
filename clears after success, and the manager reports the completed action.

Installing the same version again is a maintenance action. **Advanced → Reinstall or repair plugin**
keeps it out of the normal installation flow. **Install older version** and **Replace installed
plugin** are also explicit Advanced actions. A damaged installation offers **Repair plugin**.
The preserved previous code/data pair can also be selected through `extension.rollback`.
See [managed storage](plugin-managed-storage.md) for migration and rollback behavior.

### Recover a plugin that cannot load

Broken and incompatible desktop installations remain visible in Manage Plugins with their
load error, **Choose update file…** and **Remove**. Choose a compatible package and review
its permissions; a newer version offers **Update plugin**, and the same version offers
**Repair plugin**. Recovery never executes the rejected plugin. Other installations and
their settings are unchanged; removal keeps the selected plugin's managed data by default.

`extension.list` reports these entries in a separate `unavailable` array with `id`, `name`,
optional `version` and `error`. They do not contribute executable commands or panels.
`extension.remove` accepts their IDs. Recovery reviews and removal remain bound to the
installation fingerprint and activation observed at discovery. A real change by another
host still requires a restart and fresh review. Do not reset the store or edit internal records.
If metadata is malformed, removal preserves a bounded backup and retains payload/data whose
ownership cannot be verified; it never deletes another plugin's files to complete cleanup.

## Opt into GitHub Releases

Add an optional `repository` and `updates` object to the plugin's embedded descriptor, or to
`descriptor` in an SDK project's `authoring.json`:

```json
{
  "repository": "https://github.com/example-org/independent-plugin",
  "updates": {
    "provider": "github",
    "asset": "Independent-{platform}.pcplugin"
  }
}
```

This fragment is additional metadata, not a complete descriptor. Rebuild the module after
changing it. `repository` must be a canonical public GitHub repository URL with an owner and
repository name. It cannot contain credentials, query strings, fragments, a `.git` suffix or
additional path segments. A repository may be supplied without opting into updates.

`updates.asset` selects one exact release filename. It accepts ASCII letters, digits, dots,
underscores and hyphens and must end with `.pcplugin` or `.pcext`. It may contain one
`{platform}` token, replaced with `windows-x86_64`, `windows-aarch64`, `linux-x86_64`,
`linux-aarch64`, `macos-x86_64` or `macos-aarch64`. Paths, wildcards and other substitutions
are rejected. Use a fixed `.pcext` filename for a portable panel or a fixed `.pcplugin`
filename when the same package applies to every supported target. Neither the example
repository nor filename is hard-coded into PhotoCraft.

Publish release versions using semantic versioning, with a matching version in the embedded
descriptor. Keep the plugin ID stable. A new package must continue to declare its own
permissions, capabilities, native target, dependencies and licenses. Existing local plugins
without this metadata remain installable; they show no automatic release source.

## Checking and preparing an update

The desktop checks opted-in installed plugins automatically after a 30-second startup delay,
then at most once every 24 hours while PhotoCraft is running. The last attempt, including a
network failure, is saved with preferences so restarting does not repeatedly contact GitHub.
The saved cooldown timestamp is internal state and has no editable Preferences control.
Checks wait while user jobs, file dialogs or an installation review are active. They are skipped
in browser, inline-job, plugin-recovery and restricted automation sessions. A changed system
clock cannot cause repeated checks or a permanently deferred future timestamp.

**Automatically check for plugin updates** in the manager (also **Preferences → Plug-ins**)
can be turned off. Agents can set `plugIns.automaticUpdateChecks` through `prefs.set`.
**Check for updates** remains available immediately, including after an offline or rate-limited
attempt. Automatic results appear in the installed list and the Plugins menu's manager entry
shows the available-update count. Errors remain distinct from an up-to-date result; automatic
checks do not open windows, beep or interrupt editing. Results are held for the current session;
after restart, a manual check can refresh them before the next daily attempt.

Checks only discover releases. They do not download or install code, start a plugin worker or
grant permissions. **Update…** refreshes the short-lived candidate, downloads the exact package
and opens the existing permission review. The user still confirms installation. Existing copies
without update metadata require one manual installation of a package that declares its release
source before either automatic or manual release discovery can find their updates.

The engine exposes the same operations to authorized command clients:

```text
extension.updates.check {"id":"org.example.plugin"}
extension.updates.check {}
extension.updates.prepare {"id":"org.example.plugin","candidate":<checked candidate>}
```

`check` reports an `updates` array with each plugin's `id`, `installedVersion` and `status`:
`available`, `upToDate`, `unsupported` or `error`. An available entry includes the release
`version` and a candidate bound to that installed plugin. An unsupported source or a network
failure is distinct from an up-to-date result. `prepare` downloads and verifies the chosen
candidate and returns the ordinary installation inspection plus the local input needed for
the confirmation step. It does not publish an installation.

The final apply reuses the existing synchronous installation transaction, including archive
extraction and managed-data preparation under the store lock. Large packages can still
pause the interface during that step; background staging and a shorter publication section
remain performance work before marketplace-scale distribution.

The current updater queries each repository's latest published stable GitHub Release. Drafts
and prereleases are excluded; it does not search older releases for a missing asset. Release
versions use semantic ordering, with an optional leading `v` on the tag. GitHub's latest-release
selection or a branch head is not installation authority. The selected asset must have the declared exact name and a usable SHA-256
digest. Size and response limits, request deadlines and bounded release discovery prevent
unbounded downloads or parsing. Preparation verifies the downloaded digest, plugin ID,
embedded release version, current API and host compatibility before presenting the package.
Installation rechecks the reviewed package hash, store identity and installed revision so a
changed package, different store or competing replacement requires a fresh review.

The initial limits are a 2 MiB metadata response, 256 assets per release, an 8 MiB raw-panel
download or a 512 MiB package download. Actual length must match the asset metadata and the
selected extension must match its file format. Connections have a 10-second timeout;
metadata requests allow 20 seconds and downloads 300 seconds. A batch check has a two-minute
budget between requests; its final metadata request can add up to 20 seconds. It captures at
most 64 configured plugins, permits at most 64 distinct sources and coalesces plugins using
the same repository.
At most 16 checked candidates and 16 prepared downloads are retained for 15 minutes, with
expiration collected by the normal job pump and subsequent update work. Four process-wide updater workers bound
concurrent network jobs. Cancellation discards results immediately, but an in-flight blocking
network request can retain its worker slot until its request deadline, up to 300 seconds for
a download. A large batch may require checking an individual plugin again to obtain a fresh
preparation candidate.
Repository API redirects are rejected. Binary redirects are bounded to three hops and
restricted to supported GitHub HTTPS asset hosts on port 443, without credentials or
fragments. The desktop timer only schedules the existing bounded background command; network
checks never run on the drawing thread. Disabling automatic checks cancels its active job;
an in-flight request may retain its worker until the existing deadline described above.

GitHub metadata and a matching digest establish the bytes downloaded from that release;
they do not establish a publisher signing identity or confine trusted native code. Added
permissions and native execution still require review. Repository or update-source changes
belong in an explicitly reviewed package. Private repositories, credentials, unattended
installation, prerelease-channel selection and publisher signatures are not implemented.
Manual/offline installation remains available when GitHub is unavailable or rate-limited.

## Release checklist

1. Keep the plugin's independent source, dependency locks and notices complete.
2. Bump the embedded release version, build and test each claimed target.
3. Publish the matching GitHub Release and exact asset name with its SHA-256 digest.
4. Check discovery and preparation against an older installation in a disposable store.
5. Verify access review, cancellation/failure recovery, managed-data migration and rollback.

The [marketplace plan](plugin-marketplace.md) describes future discovery and publisher trust.
It does not add another packaging format, API major or plugin runtime dependency.
