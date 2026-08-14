# Inventory Curation - Requirements Discussion

**Status:** ready-for-prd
**Release:** v2.0.1
**Feature:** inventory-curation
**Last updated:** 2026-08-11

## Problem

The Project Tree retains many one-off Projects and Sessions because their Provider source
logs remain available. Recent and Older disclosures therefore grow continually, and the
Tree gives too much visual weight to work that the developer no longer considers relevant.
Project ordering cannot solve this because the unwanted inventory remains present.

## Users and Outcomes

The local developer should be able to keep the default Tree focused on work they still care
about without deleting Provider-native history or losing the ability to recover an item that
was hidden by mistake.

## Current Experience

- Provider JSONL logs are the source of truth. A Session remains indexed while its source
  log exists and returns after a disposable-index rebuild.
- Only Sessions whose `cwd` is under `/tmp`, `/private/tmp`, `/var/folders`, or
  `/private/var/folders` are excluded as system-temporary work.
- A Project is Current when it has an Open Session or its newest Session is less than seven
  days old. Other Projects are grouped under the collapsed Older Projects section.
- Within a visible Project or Worktree, Open Sessions are direct rows and Exited Sessions
  are grouped into Recent and Older disclosures.
- The inspected local index contained 326 Sessions across 48 distinct working directories:
  148 Recent, 178 Older, and only 16 covered by the current system-temporary exclusion.

## Scope

### In Scope

- A developer-controlled way to reduce irrelevant Project and Session rows in the default
  inventory.
- Durable visibility choices that survive Mudi restart and disposable-index rebuild.
- A recoverable way to inspect and restore hidden inventory.
- Consistent visibility semantics across the normal Overview inventory.

### Out of Scope

- Deleting or modifying Claude Code or Codex CLI source logs.
- Treating deletion of the disposable Mudi index as a visibility preference.
- Inferring that Provider-native history is safe to delete.
- Changing Project identity, Session identity, or Project ordering rules.

## Required Behaviour

- Existing system-temporary directory exclusions remain automatic.
- All other curation is developer-directed through an explicit Hide action. Mudi does
  not infer irrelevance from a path name, Session count, activity age, or similar heuristic.
- Both Project-level and Session-level Hide actions are available. Session Hide targets one
  native Session identity. Project Hide is a snapshot bulk action over the currently
  observed Sessions in that Project; it creates no Project-level exclusion.
- A Session discovered later in the same Project is visible by default and causes the
  Project to reappear in the Tree. Sessions hidden by the earlier bulk action remain hidden.
- Project Hide confirmation identifies the number of current Sessions being hidden so the
  action does not imply that future Sessions will also be excluded.
- An Open Session is always visible even when its identity is hidden. This is a temporary
  visibility override: it participates in lifecycle and safety surfaces while Open, then
  returns to hidden after exit unless the developer explicitly removes it from Hide.
- Only a validated Open lifecycle state overrides Hide. A newer activity timestamp, delayed
  Provider log write, historical rescan, or other metadata refresh does not make an Exited
  hidden Session visible. A distinct native Session identity remains a new visible Session.
- Hidden inventory does not appear as a section or disclosure in the normal Project Tree.
  The full Session Detail opened with Enter provides one contextual `Hidden List` entry,
  and a global top-level shortcut provides direct access without requiring a visible or
  selected Session.
- Both entry points open the same hidden-inventory management view. That view presents all
  hidden Sessions using a Project Tree hierarchy rather than a flat list.
- Hidden Tree does not reproduce Recent or Older disclosures. Within each Project and
  Worktree, hidden Sessions are direct rows sorted by latest activity, newest first.
- Hidden Tree opens with every Project collapsed after Mudi startup. Up and Down move
  selection; Enter toggles the selected Project open or closed. Left and Right are not part
  of this view. Expansion state may persist only for the current application run and resets
  to collapsed after restart.
- On a top-level page with no Detail open, `h` is the global shortcut for the Hidden Tree.
  Inside Session Detail, the existing `h` Handoff action remains unchanged and the
  selectable Hidden List entry uses Enter.
- When no hidden Sessions exist, Session Detail omits the Hidden List entry and top-level
  `h` shows `No hidden sessions` without opening an empty view. The entry appears after the
  first successful Hide and displays the hidden count.
- In the normal Project Tree, uppercase `H` requests Hide for the selected Project or
  Session. Project Hide confirms the number of Sessions affected; Session Hide confirms
  the selected Title and Provider. Worktree rows do not expose Hide.
- Hide is available only for Exited Sessions. An Open Session remains unchanged and receives
  a brief `Open Session cannot be hidden` notice. Project Hide targets only its currently
  observed Exited Sessions and its confirmation separately reports the number to hide and
  the number of Open Sessions that will remain visible.
- The Hidden List supports restoring an item by removing it from Hide and has a distinct
  Delete action.
- `Remove from Hide` restores the item to the normal inventory. `Delete` permanently
  excludes the item from Mudi inventory and removes its Session Alias and
  Description, while retaining only the minimum durable exclusion identity needed to
  prevent a later source scan from importing it again.
- Delete never deletes or modifies Claude Code or Codex CLI source logs.
- Delete does not remove or mutate Managed Session or Binding evidence, Operation Tasks,
  Handoff Artifacts, or Task/Successor relationships. Those records keep their independent
  lifecycle and deletion rules.
- In the Hidden Tree, `u` removes the selected Session from Hide and `d` requests confirmed
  permanent deletion from Mudi inventory. On a Project row these actions apply in
  bulk to every currently hidden Session in that Project and disclose the affected count.
  Worktree rows remain structural and expose neither action.
- Esc closes the Hidden Tree and restores the exact originating Overview selection.
- Hidden Exited Sessions are absent from the normal Overview Project Tree. The same Open
  override applies there. Hidden Tree is the only inventory that shows hidden Exited Sessions.
- Overview Project, Session, and Usage totals are calculated from the same visible
  inventory and exclude hidden Exited Sessions. Hidden Tree reports its own Project and
  Session counts. Hiding changes presentation scope only; source logs and indexed facts
  remain intact.
- Session-scoped Recent Signals are hidden when their Session is hidden and Exited. They may
  reappear after Remove from Hide only while still retained by the existing five-Signal
  process-lifetime policy. Open override makes applicable Signals visible. Shared Worktree
  Risk remains based on Open Sessions and is never suppressed by historical hiding.
- Each durable curation record is keyed only by Provider plus native Session ID and has one
  disposition: Hidden or Permanently Excluded. No Project-level curation record is stored.
  Existing installations migrate with every Session visible by default.
- A durable record whose Provider source is currently unavailable is omitted from Hidden
  Tree; there is no Unavailable group. The disposable index may prune the absent Session,
  but Hide/Permanent Exclusion, Alias, and Description remain dormant. If source evidence
  returns, the prior disposition and metadata apply again.

## Edge and Failure Cases

- A hidden Project later receives a new or Open Session.
- A hidden Session is precisely resumed or becomes Open through external activity.
- A hidden Project temporarily disappears from Provider evidence and later returns.
- Project identity changes because a directory is moved, recreated, or changes between Git
  and non-Git resolution.
- A saved visibility record refers to inventory that is no longer observable.
- Hidden items contribute to counts, Attention Conditions, Recent Signals, or search/filter
  results differently from visible items.

## Constraints and Dependencies

- Provider source logs remain authoritative for observed Session facts.
- The SQLite Session index is disposable, so durable curation metadata cannot live only in
  that index.
- Curation identity follows the existing Session identity contract: Provider plus native
  Session ID. It does not persist a Title, `cwd`, Project label, or Project identity as the
  matching key.
- Permanent exclusion and Alias/Description removal must commit as one authoritative local
  metadata change so a partial failure cannot erase metadata while allowing re-import, or
  exclude a Session while retaining metadata that Delete promised to remove.
- A missing legacy curation field means Visible. An unreadable or unsupported durable
  metadata store must disable curation edits and report degradation rather than guessing
  visibility or overwriting user metadata.
- Visibility must not silently grant or remove control authority over a Managed Session.
- Open Sessions and Shared Worktree Risk may require safety treatment distinct from Exited
  historical inventory.
- Adding this capability to v2.0.1 changes an accepted version boundary and must be made
  explicit before delivery.

## Decisions

- The discussion belongs to release `v2.0.1` under feature slug `inventory-curation`.
- The problem is inventory relevance, not Project ordering or deletion of Provider history.
- Apart from the existing narrow system-temporary exclusions, hiding is explicit and
  developer-controlled; Mudi will not automatically classify ordinary work as
  temporary.
- Rejected a collapsed Hidden section in the Project Tree. Hidden inventory is managed from
  a dedicated Hidden Tree reached from full Session Detail or a global shortcut.
- `Details` means the full Session Detail opened with Enter, not the compact DETAILS panel
  below the normal Project Tree.
- The global Hidden Tree shortcut is `h` on top-level pages. Its existing Session Detail
  meaning remains Handoff, so the two actions are separated by view context.
- Empty hidden inventory has no Session Detail entry; global `h` reports a brief empty-state
  notice instead of opening Hidden Tree.
- Uppercase `H` is the normal-Tree Hide command for Project and Session selections. It is
  confirmation-gated, and Worktree is not a hideable scope.
- Open Sessions cannot be hidden. Project bulk Hide skips Open Sessions and discloses both
  affected Exited and retained Open counts before authorization.
- Hidden Tree uses `u` for Remove from Hide and `d` for confirmation-gated permanent
  Mudi deletion. Project selection applies either operation to its current hidden
  Sessions as a disclosed bulk action; Worktree remains structural.
- Esc from Hidden Tree restores the originating Overview selection.
- Hide applies consistently to the normal Overview inventory. Hidden Exited Sessions appear
  only in Hidden Tree; a validated Open override surfaces them in Overview.
- Overview Project, Session, and Usage totals exclude hidden Exited Sessions so displayed
  summaries match the visible inventory. Hidden Tree has separate counts.
- Recent Signals follow visible inventory for hidden Exited Sessions. Open lifecycle and
  Shared Worktree Risk safety behavior continue to surface normally.
- Hidden Tree uses a simplified Project/Worktree/Session hierarchy with direct Session rows
  ordered newest first and no age-band disclosures.
- Hidden Tree uses Up/Down navigation and Enter-only Project expansion. Projects start
  collapsed after restart; expansion state is process-lifetime only.
- The Hidden List offers both Delete and Remove from Hide as distinct actions.
- Delete means permanently ignore within Mudi, not delete Provider-native history.
  Remove from Hide means restore the item to normal inventory.
- Permanent inventory Delete removes Alias and Description but preserves managed-control
  evidence, Operation Tasks, Handoff Artifacts, and Successor relationships.
- Both Project and Session rows support Hide. Session Hide affects only the selected native
  Session.
- Project Hide expands to the Project's currently observed Session identities. It does not
  persist a Project exclusion, so a newly discovered Session makes the Project visible
  again without restoring the previously hidden Sessions.
- Open overrides Hide without clearing the Hide preference. The Session becomes visible and
  participates in Busy/Idle, Attention, and Shared Worktree Risk while Open, then returns to
  hidden after exit.
- Metadata activity alone never overrides Hide; only a validated Resume or other Open
  Current Run for the same native Session does. A newly created native Session remains
  visible by default.
- Durable visibility uses Provider plus native Session ID, matching the repository's Session
  identity. Project bulk actions persist only their resolved Session identities, and legacy
  data migrates to Visible by default.
- Source absence creates no visible Unavailable row and does not erase durable curation or
  user-authored Session metadata. Returning source evidence restores the prior state.

## Open Questions

None at the current product boundary.

## Evidence

- `CONTEXT.md` defines Provider JSONL as source of truth and the Session index as disposable.
- `src/ui/project_tree/model.rs` groups all resolved non-system-temporary Sessions into the
  Project Tree and applies the seven-day Recent/Older cutoff.
- `src/ui/provider.rs` uses the same system-temporary exclusion and seven-day history split.
- `src/ui/mod.rs` recognizes only the four system-temporary roots listed above.
- `src/index/mod.rs` prunes a Session only when its source file vanishes.
- `src/domain.rs` defines Session identity as Provider plus native Session ID.
- `src/session_labels.rs` already provides versioned, owner-only, atomic local Session
  metadata updates and degraded behavior for unreadable or unsupported stores.

## Session Log

### 2026-08-11

- Recorded the reported Tree clutter and inspected the current inventory behavior and local
  index composition.
- The developer assigned the discussion to `v2.0.1`.
- Established `inventory-curation` as the discussion slug; no visibility workflow has been
  accepted yet.
- Confirmed that ordinary Project and Session hiding is always explicit; only the existing
  system-temporary roots remain automatically excluded.
- Rejected a Hidden Tree section. Hidden inventory will be managed through a Hidden List
  entry in Details, with Delete and Remove from Hide actions whose exact deletion boundary
  remains to be defined.
- Defined Delete as a Mudi-local permanent exclusion that removes Mudi-owned
  metadata and keeps only a minimal exclusion identity. Provider-native source logs remain
  untouched; Remove from Hide restores normal visibility.
- Confirmed that both Projects and individual Sessions expose Hide. Project Hide may be a
  bulk action over current Session identities rather than a durable Project exclusion; that
  future-Session behavior is the next decision.
- Confirmed Project Hide as a snapshot bulk action over current Session identities. Future
  Sessions in that Project remain visible and make the Project reappear.
- Confirmed that Open lifecycle state temporarily overrides Hide without removing the Hide
  record. The Session remains part of active lifecycle, Attention, and shared-Worktree safety
  behavior, and becomes hidden again after exit.
- Corrected the destination: the contextual entry belongs to the full Session Detail opened
  with Enter, not the compact DETAILS panel. A separate global shortcut will open the same
  all-hidden management view, rendered with Tree hierarchy.
- Selected `h` as the top-level global Hidden Tree shortcut. Session Detail keeps its
  existing `h` Handoff behavior and uses the selectable Hidden List entry for access.
- Selected uppercase `H` for confirmation-gated Hide on normal Project and Session rows;
  Worktree remains structural and cannot be hidden directly.
- Selected `u` and `d` for Hidden Tree restoration and confirmed permanent exclusion.
  Project rows apply them in bulk to current hidden Sessions, Worktree rows remain
  structural, and Esc restores the origin.
- Confirmed that only a validated Open lifecycle state temporarily overrides Hide. Metadata
  activity and rescans do not; a distinct native Session identity is visible as new work.
- Confirmed that Hide removes Exited Sessions from the normal Overview inventory. Hidden Tree
  is their management surface; Open override behavior remains unchanged.
- Confirmed that Overview Project, Session, and Usage totals use visible inventory only;
  Hidden Tree reports separate counts while underlying evidence is unchanged.
- Confirmed that Recent Signals for hidden Exited Sessions are filtered with the inventory,
  while Open override and Shared Worktree Risk remain visible safety behavior.
- Confirmed that Hidden Tree omits Recent/Older disclosures and directly lists hidden
  Sessions newest first within the Project/Worktree hierarchy.
- Confirmed Enter-only Project expansion with Up/Down navigation in Hidden Tree. Projects
  reset to collapsed on restart; no Left/Right behavior is added.
- Confirmed that only Exited Sessions can be hidden. Project bulk Hide skips and reports
  Open Sessions; direct Hide on Open shows a brief explanatory notice.
- Confirmed the empty state: no Detail entry at zero hidden Sessions, and global `h` reports
  `No hidden sessions` without opening an empty view.
- Confirmed that permanent inventory Delete removes only Alias/Description and normal
  inventory presence. Managed and Binding evidence, Tasks, Handoff Artifacts, and Successor
  relations preserve their independent lifecycles.
- Derived durable identity and migration behavior from existing repository contracts:
  Provider plus native Session ID, no Project exclusion record, existing data Visible by
  default, and no visibility guessing on store failure.
- Confirmed that source-unavailable hidden records are absent from Hidden Tree without an
  Unavailable group. Durable curation and user metadata remain dormant and resume if the
  Provider source returns.
- The developer confirmed that the analysis is complete and ready to become a formal PRD.
