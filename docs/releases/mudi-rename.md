# Mudi rename release notes

Mudi is the new product and technical identity for the application formerly named
SessionMesh. The Cargo package, library crate, primary binary, default run target,
documentation, UI, generated commands, private identifiers, and managed Handoff Skill
now use `mudi` or `Mudi`. The GitHub repository is expected at
`ZhangPeibin/mudi`; the repository rename is a coordinated maintainer action that must
complete before publishing this release.

## Storage selection

Mudi does not copy or merge application data during this rename. This avoids breaking
absolute Handoff Artifact references and avoids duplicating or overwriting authoritative
Task and Managed stores.

| Default-directory state | Selected directory |
|---|---|
| Neither exists | `~/Library/Application Support/mudi` |
| Only `sessionmesh` exists | Existing `sessionmesh` directory, in place |
| Only `mudi` exists | Existing `mudi` directory |
| Both exist | Startup stops; set `MUDI_DATA_DIR` explicitly |

Selection is idempotent. A legacy-only installation continues reading and writing the
same complete directory, so its SQLite index, Task Store, Managed Launch Store, Session
labels, Profile, model overrides, Handoff Artifacts, Managed Sessions, and Binding
evidence remain available together. Mudi never silently chooses between two existing
default directories.

## Compatibility period

The following compatibility paths remain supported throughout the v2.x line:

- The `sessionmesh` executable invokes the same application as `mudi`, including
  previously copied `sessionmesh attach <opaque-ref>` commands.
- `SESSIONMESH_*` variables remain fallbacks for their `MUDI_*` replacements. `MUDI_*`
  wins when both are set; differing values produce a warning. Existing `TOKENPLAY_*`
  directory fallbacks remain lowest precedence.
- Stored private tmux socket and session names are used exactly as recorded, so existing
  `sessionmesh-*` environments remain attachable. Newly created environments use
  `mudi-*`.
- Saved `sessionmesh` and `tokenplay` Theme names load as the canonical `mudi` Theme.
- New authorizations install and invoke `mudi-handoff`. Existing owner-only managed
  `sessionmesh-handoff` Skills remain valid Worker fallbacks and are not overwritten or
  removed.

Any removal of these compatibility paths requires a separately agreed version scope,
GitHub Issue, migration review, and release note. This rename does not schedule their
removal.
