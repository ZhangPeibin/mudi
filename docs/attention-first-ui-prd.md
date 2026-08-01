# SessionMesh Attention-First UI PRD

> Living document. This PRD captures the proposed product direction discussed after v1.
> New requirement points should be reviewed here and marked with an explicit decision
> status before they are treated as implementation scope.

| Field | Value |
|---|---|
| Status | Draft |
| Last updated | 2026-08-01 |
| Product phase | Post-v1 exploration |
| Related baseline | `docs/spec.md` |
| Production render evidence | `tests/fixtures/production-renders/` |

## 1. Document contract

This document is the working source for the attention-first product direction. It does
not supersede the v1 specification until the relevant requirements are confirmed.

Decision statuses:

- **Proposed** - captured for discussion; not implementation scope yet.
- **Confirmed** - agreed product behavior; eligible for planning and implementation.
- **Deferred** - valuable, but intentionally postponed.
- **Rejected** - considered and explicitly excluded, with the reason recorded.

When a new requirement is confirmed, update its status, acceptance criteria, affected
screens, and the decision log in the same change.

## 2. Background

The v1 UI is primarily a monitoring dashboard. It shows provider totals, session state,
context pressure, historical usage, estimated cost, and compaction history.

The higher-value user problem is not merely observing those metrics. A user running
several Claude Code and Codex CLI sessions needs to answer:

> Which coding session needs my attention now, why, and what should I inspect next?

Provider-centric navigation makes it harder to see cross-provider relationships. In
particular, two sessions from different providers may affect the same project or writable
Git worktree. A metrics-first dashboard also makes the user interpret raw state before
deciding what to do.

## 3. Product position

SessionMesh should evolve from a token and session monitor into a local attention console
for concurrent AI coding sessions.

The product promise is:

> See where intervention is needed, where concurrent agents create Shared Worktree Risk,
> and what changed while you were away, without storing prompt or source-code content.

## 4. Goals

- Make the most important current issue visible immediately after launch.
- Explain why every Attention Condition exists using observable evidence.
- Reveal project and worktree relationships across providers.
- Support investigation of recent state changes without reading prompt content.
- Preserve the local, metadata-only, read-only trust model.
- Keep the interface dense, keyboard-first, and suitable for repeated terminal use.

## 5. Non-goals

- Automatically resume, terminate, compact, or modify provider sessions.
- Read or store prompts, responses, tool calls, tool outputs, or source-code contents.
- Present heuristic classifications as facts without evidence.
- Replace the complete session list or usage detail views.
- Turn the default screen into a cost analytics dashboard.
- Add remote telemetry as a dependency for measuring product usefulness.

## 6. Primary navigation and help

### NAV-001 - Three numeric primary pages

**Status: Confirmed**

The persistent top-level navigation contains exactly three numeric targets:

```text
[1]  [2]  [3]
```

| Key | Page |
|---:|---|
| `1` | All - the current main/global page |
| `2` | Claude Code sessions |
| `3` | Codex CLI sessions |

The persistent navigation must not render `All`, `Claude Code`, or `Codex CLI` beside the
numbers. The active number is visually highlighted. Page content may identify its
provider where needed; the restriction applies to the persistent numeric navigation.

Project and Worktree organization is part of `NOW`, not a separate navigation target.
Full Signal History is Deferred and is not a top-level page, detail section, overlay, or
main-page region in the first attention-first implementation.

### NAV-002 - Introductory help overlay

**Status: Confirmed**

At the beginning of use, SessionMesh presents a small dismissible Help overlay that explains
the `1/2/3` mapping and essential keyboard controls. This keeps the persistent navigation
compact without making the numeric mapping undiscoverable.

The overlay opens automatically only on the first launch for a local Profile. Dismissing
it stores a `help_seen` preference outside the disposable Session index; later launches do
not auto-open it, and deleting that local preference restores first-launch behavior. `?`
reopens Help at any time.

## 7. Global interaction principles

### UI-001 - Decision before metrics

**Status: Confirmed**

The first viewport prioritizes current Attention Conditions and evidence. The main screen
does not display Processed Usage, Estimated Cost, or Provider aggregate usage/cost.
Current Context remains visible because it is current operational state, not cumulative
Processed Usage. Usage and cost remain available on Provider pages and in Session detail.

### UI-002 - Progressive disclosure

**Status: Confirmed**

Information is revealed in this order:

```text
Attention Condition -> reason/evidence -> Session detail -> complete Usage
```

The list must contain enough evidence to trust the classification without requiring the
detail view for every item:

- a Context Alert shows the stable-width Context progress bar, exact percentage, and the
  threshold it crossed;
- Shared Worktree Risk appears at the Worktree level with the count of contributing Open
  Sessions; the contributing Session rows immediately below are the member evidence and
  are not repeated in a second list;
- Idle shows elapsed time since the observed Busy-to-Idle transition, falling back to last
  activity when that transition time is unavailable;
- healthy Busy shows Current Context and last activity with lower visual emphasis.

### UI-003 - One primary action per selected item

**Status: Confirmed**

The selected row exposes one primary command, normally `Inspect`. Secondary commands such
as filtering must not compete visually with it. `Acknowledge` and `Snooze` are Deferred
and do not appear as commands in the first attention-first implementation.

### UI-004 - Healthy state stays quiet

**Status: Confirmed**

Healthy running sessions remain visible in a lower-emphasis roster or inventory, but must
not compete with critical or warning items.

### UI-010 - Idle is neutral Attention

**Status: Confirmed**

Every Idle Open Session has a neutral Attention Condition because its provider process is
awaiting input. Idle is not by itself a Warning or Critical Alert: it does not use the alert
palette and does not trigger a system notification. It ranks below Warning conditions and
above healthy Busy Sessions according to `UI-011`.

### UI-011 - Cross-kind Attention priority

**Status: Confirmed**

Current conditions use the following descending priority:

1. Critical Context Alert (`Current Context >= 85%`);
2. Shared Worktree Risk;
3. Warning Context Alert (`Current Context` from 70% through 84%);
4. neutral Idle Attention;
5. healthy Busy with no Attention Condition.

A Session or Project may carry multiple conditions. Its ordering rank is determined by
the highest-priority current condition while the UI retains the lower-priority evidence.
Lower-priority conditions do not alter the ordering rank.

Items with the same rank use condition-specific evidence strength first:

- Context Alerts sort by Current Context percentage, highest first;
- Shared Worktree Risks sort by contributing Open Session count, highest first;
- Idle Sessions sort by the time they most recently entered Idle, newest first;
- healthy Busy Sessions sort by last activity, newest first.

Remaining ties use the most recent relevant transition or activity, then canonical Project
and Worktree path, then Provider plus native Session ID. If an Idle transition timestamp
is unavailable, last activity is the permitted ordering fallback. These rules form a
visible, deterministic lexicographic order; SessionMesh must not replace them with an opaque
weighted score.

## 8. Main screen (`1`)

### 8.1 Purpose

The All screen (`1`) is a compact terminal workspace rather than a full-screen Session
table or a large summary surface. It gives the user a current-session inventory and a
small record of meaningful recent changes without duplicating the same Sessions in a
separate Attention queue.

### 8.2 Layout

**Status: Confirmed at region level**

The terminal may be divided into multiple regions, but only two regions currently have
confirmed product responsibilities:

| Region | Status | Responsibility |
|---|---|---|
| `NOW` | Confirmed | Open Sessions, grouped by Project |
| `RECENT SIGNALS` | Confirmed | A short list of meaningful recent metadata changes |
| Reserved region A | Unassigned | Remains empty until a user job is confirmed |
| Reserved region B | Unassigned | Remains empty until a user job is confirmed |

The two reserved regions must not be labelled or implemented as `FOCUS`, `PROJECT PULSE`,
Signal History, or another feature merely because those concepts appeared in a wireframe.
Their placement and purpose remain open.

### 8.3 `NOW`

**Status: Confirmed**

- Show only Projects that contain at least one Open Session.
- Organize Open Sessions as `Project -> Worktree -> Session` while preserving one
  selectable row per Session.
- Identify a Git Project by canonical Git common directory and a Worktree by canonical
  Git top-level path. Linked Worktrees belong to one Project; independent clones remain
  distinct even when they share a remote URL.
- Place non-Git Sessions directly in a standalone Project identified by canonical `cwd`,
  with Git-only fields unavailable.
- Show branch, Worktree root, Shared Worktree Risk, and highest Context pressure at the
  Worktree level. Keep state, Provider, model, Current Context progress, and last activity
  scannable in each Session row.
- Render Current Context as the stable-width progress bar plus percentage defined by
  `UI-008`; only compact-width degradation may reduce it to the percentage alone.
- Order Projects by current Attention priority, with the highest-priority Project first.
- Apply the same confirmed Attention order within each Project and Worktree; the first
  Session row is the recommended inspection target.
- Do not apply a time filter on the main screen.
- Do not duplicate Sessions in a separate Attention list.
- Do not display Processed Usage, Estimated Cost, or Provider aggregate usage/cost.
- Context threshold colors follow `UI-008`.

### 8.4 Shared Worktree Risk

**Status: Confirmed**

A Shared Worktree Risk exists when two or more distinct Open Sessions resolve to the same
writable Git Worktree. Busy and Idle Sessions are Open; Exited Sessions do not contribute.
The count uses provider-native Session identity, not the number of discovered source-log
paths.

Sharing only a Project while using separate Worktrees does not create this risk. If
Worktree identity cannot be resolved, the state is unknown and SessionMesh must not infer a
risk. Shared Worktree Risk reports structural exposure only; it does not claim that the
Sessions edit the same files or that a file-level or Git merge conflict has occurred.

Shared Worktree Risk always has Warning severity in the first version. The number of
contributing Open Sessions is displayed as evidence but does not by itself escalate the
risk to Critical.

### 8.5 `RECENT SIGNALS`

**Status: Confirmed**

`RECENT SIGNALS` is a compact, newest-first list answering "what just changed?" It is
the only event-list capability in the first attention-first implementation. Full Signal
History, filters, search, a timeline inspector, and long-term retention are Deferred.

Recent Signals are process-lifetime only. The initial scan establishes a baseline and
does not emit Signals for conditions that already exist. Only changes observed after that
baseline are recorded; quitting SessionMesh clears the list, and the next launch starts with
a new empty list. Current conditions discovered at launch still appear immediately in
`NOW`.

The first version records these Recent Signals:

- Busy changed to Idle;
- Busy or Idle changed to Exited;
- Exited changed to Busy or Idle when a Session is opened or resumed;
- Current Context entered or cleared Warning or Critical;
- Compaction observed;
- Shared Worktree Risk detected or cleared.

It does not record Idle-to-Busy changes, Busy heartbeats, ordinary Context percentage
changes, Usage or Cost changes, source-file timestamp changes, unchanged polling
observations, or `Possible Stall`.

When one Compaction observation also clears a Context Alert, SessionMesh emits one compound
Recent Signal containing the Compaction evidence, Context before/after values, and cleared
Alert rather than occupying multiple rows.

`RECENT SIGNALS` retains at most the five newest Signals for the current SessionMesh process
and renders them newest first. A compact viewport may render fewer than five when required
to preserve `NOW`; the region does not scroll, expand, or link to additional history. A
new Signal is inserted at the top and evicts the oldest retained Signal when the limit is
already reached.

`Idle` is a current Session state meaning that the provider process is open and awaiting
input, and it carries a neutral Attention Condition. A transition from Busy to Idle is a
Recent Signal; repeated observations of an unchanged Idle Session are not new signals.

### 8.6 Main-screen acceptance criteria

- `NOW` does not consume the entire terminal when sufficient width and height are
  available for multiple regions.
- Every Open Session appears exactly once in `NOW`.
- Exited Sessions do not appear in `NOW`.
- A transition to Exited may appear in `RECENT SIGNALS` but is not a current Attention
  Condition.
- Sessions from different Providers appear together when they belong to the same Project.
- Linked Worktrees appear under one Project and remain distinguishable.
- Independent clones remain separate Projects even when their remote URL is the same.
- Shared Worktree Risk evidence identifies the Worktree and every contributing Open
  Session without reading source-code content.
- Shared Worktree Risk clears when fewer than two distinct Open Sessions resolve to the
  Worktree.
- Attention ordering and Context colors make the highest-priority current Project
  scannable.
- The main screen contains no Processed Usage or Estimated Cost values.
- `RECENT SIGNALS` is visually and semantically distinct from current Session state.
- `RECENT SIGNALS` retains at most five entries, orders them newest first, and provides no
  scrolling, expansion, or `View more` action.
- A compact viewport reduces visible Recent Signals before reducing the space required to
  understand `NOW`.
- `Tab` and `Shift+Tab` move focus between `NOW` and `RECENT SIGNALS`; the focused region
  is visibly identified and each region retains its own selected row.
- `Enter` on a Session-scoped Recent Signal opens that Session's full-page Detail.
- `Enter` on a Shared Worktree Risk Signal switches to page `1` and focuses the matching
  Worktree in `NOW`. If the risk has cleared and the Worktree no longer contains an Open
  Session, SessionMesh stays on the Signal and briefly reports
  `Worktree is no longer open` in the status line.
- Duplicate polling observations do not produce duplicate Recent Signals.
- A threshold crossing produces one Recent Signal per crossing and may produce another
  when the condition clears.
- A Compaction and the Context Alert it clears appear as one compound Recent Signal.
- Recent Signals never contain prompt, response, tool, or source-code content.
- Restarting SessionMesh starts `RECENT SIGNALS` empty without affecting current conditions
  shown in `NOW`.
- The two remaining regions have no product label or behavior until separately confirmed.

## 9. Provider session pages

### 9.1 Purpose

Pages `2` and `3` preserve the complete v1 inventory and detail workflow for Claude Code
and Codex CLI respectively. They are the factual views behind attention classifications.

### 9.2 Navigation behavior

**Status: Confirmed**

- `2` always opens the Claude Code session list.
- `3` always opens the Codex CLI session list.
- Sessions are grouped by Project identity within each Provider page.
- Each session is rendered as exactly one selectable row inside its project group.
- Project is the organization unit; Session remains the inspection and detail unit.
- Order Project groups by the most recent `last_activity` of any contained Session,
  newest first.
- Preserve state, project, model, context, estimated cost, update time, and compaction
  columns.
- Allow entry from `NOW` and Session-scoped Recent Signals with the relevant Session
  selected. Worktree-scoped Signals locate their Worktree on page `1` instead of choosing
  an arbitrary contributing Session.
- Do not render a Time Filter control.
- Inside each Project, render Sessions with last activity inside the rolling seven-day
  window directly, ordered by `last_activity` newest first.
- Under each Project, render Sessions before that cutoff in that Project's own
  `OLDER SESSIONS` disclosure, collapsed by default, showing its Session count, and ordered
  by `last_activity` newest first when expanded.
- Do not create global `LAST 7 DAYS` or `OLDER SESSIONS` sections outside Project groups.
- Apply the cutoff strictly by last activity. An Open Session older than seven days remains
  visible in `NOW` and belongs to `OLDER SESSIONS` on its Provider page.
- Keep a Project visible when it contains no Recent Sessions; its collapsed
  `OLDER SESSIONS` disclosure is the Project's only child row.
- Every row displays the Session's complete lifetime Usage; the seven-day boundary never
  splits or limits Usage.

### 9.3 Row-organization acceptance criteria

- Every visible session appears once and opens its own Session detail.
- Sessions belonging to the same Project are visually grouped.
- Distinct Projects with the same display basename remain separate and receive
  disambiguated labels.
- A project group may contain multiple sessions without merging their state, context,
  model, usage, or provider-native identity.
- Direct Recent rows and per-Project `OLDER SESSIONS` disclosures partition the Provider's
  Sessions without omission or duplication.
- Each Project's `OLDER SESSIONS` is collapsed on first entry to a Provider page and can
  be expanded independently without navigating away.
- Expanding or collapsing `OLDER SESSIONS` does not change Session data or Usage scope.

## 10. Session detail

**Status: Confirmed**

Session detail remains the evidence surface and should contain:

- provider, native session ID, project, cwd/worktree, model, and current state;
- current context occupancy and threshold state;
- Current Run Usage and Transcript Lifetime Usage as separate, explicitly labelled scopes;
- per-model usage within the selected scope;
- compaction timeline;
- current Attention Conditions and the evidence behind each.

Session Detail does not repeat `RECENT SIGNALS`, add a Recent Transitions list, or expose
a Signal timeline. Compactions remain because they are provider-recorded transcript
metadata that can be rebuilt independently of SessionMesh's process-lifetime Signals.

### UI-013 - Full-page Session Detail

**Status: Confirmed**

`Inspect` opens Session Detail as a full-page child view in the content area, not a modal
or centered overlay. The persistent `[1] [2] [3]` navigation remains visible and does not
gain a fourth target.

- `Enter` from a Session row in `NOW`, a Session-scoped Recent Signal, or either Provider
  page opens the same Session Detail identified by Provider plus native Session ID.
- `Esc` returns to the originating page with its selection, scroll position, and expanded
  `OLDER SESSIONS` disclosures restored.
- `1`, `2`, or `3` leaves Detail and navigates directly to that top-level page.
- Detail scrolls vertically with `Up/Down` or `j/k` when its content exceeds the viewport.
- The content identifies the Provider, Project, and Session so the user retains location
  context without relying on a modal backdrop.

### UI-009 - Current-run and transcript-lifetime usage

**Status: Confirmed**

Session detail must distinguish the usage produced by the current provider invocation
from usage accumulated across the complete native Session transcript:

- `Current Run Usage` is the primary comparison surface for an open Claude Code Session
  and should match Claude Code `/usage` within the provider's display rounding.
- `Current Run Usage` exists only for an Open Session. An Exited Session displays no
  `Current Run Usage` or `Last Run Usage` because it has no current provider invocation.
- `Transcript Lifetime Usage` includes inherited history from earlier Runs of the same
  native Session. It remains available for both Open and Exited Sessions because the
  complete JSONL transcript remains reconstructable across resume operations.
- The UI must never add the two scopes together or present either one as an unqualified
  `Total Usage` or `Session Usage`.
- Current Run Usage is bounded by the provider Run boundary. For an open Claude Code
  Session, the authoritative boundary is the live state record's process `startedAt`.
- Current Run Usage includes every model used after that boundary. Filtering by the
  latest model is not an acceptable substitute for identifying the Run boundary.
- Input, output, cache read, and cache creation remain separate in both scopes.
- Estimated API cost must identify which usage scope it estimates and must not imply that
  it equals the provider's subscription charge.
- If the Current Run boundary cannot be established, display Current Run Usage as `N/A`
  rather than guessing from model names, file modification time, or the latest records.
- Do not persist a completed Run boundary solely to retain its Usage after exit. Do not
  infer a Last Run boundary from timestamp gaps, file offsets, or task/turn events.
- Provider model names continue to come from source metadata. A model alias shown by a
  provider command must not be silently substituted without a confirmed mapping.

### UI-007 - Source metadata

**Status: Confirmed**

Session detail should expose the exact local source metadata needed to locate a session:

- working directory (`cwd` / process `pwd`) when known;
- provider source-log path;
- source format (`JSONL` for current Claude Code and Codex CLI adapters);
- native provider session ID.

The current implementation already indexes the source-log path and carries `cwd`, but the
normalized UI `Session` does not retain the source path. Session Detail displays complete
paths even when compact lists truncate them. SessionMesh must not read or display raw JSONL
contents, prompt/response content, tool content, or source-code content through this view.

Pressing `c` in Session Detail opens a Copy menu containing available values for source-log
path, Worktree path, `cwd`, and native Session ID. `Up/Down` or `j/k` selects a field,
`Enter` writes its complete untruncated value to the macOS clipboard, and `Esc` cancels.
Unavailable values are disabled. The copied value contains no field label or trailing
newline; success or failure appears briefly in the status line. The Copy command never
reads or copies raw JSONL or source-code content.

## 11. Visual language

### UI-005 - Theme-driven semantic color

**Status: Confirmed**

Components consume semantic Theme roles rather than hard-coded colors. Every built-in
Theme defines at least:

- structural roles: background, foreground, muted text, border, accent, selected
  background, and selected foreground;
- lifecycle and attention roles: Critical, Warning, Idle, healthy Busy, and
  Exited/unknown;
- Context roles: normal, Warning, and Critical.

SessionMesh ships with a built-in `sessionmesh` Theme as the default. `t` cycles through the
available built-in Themes at runtime, briefly reports `Theme: <name>` in the status line,
and stores the choice as a local Profile preference outside the disposable Session index.
The next launch restores that choice.

The first implementation includes six built-in Themes in cycle order:

1. `sessionmesh` - the default dark Theme;
2. `catppuccin` - Catppuccin Mocha;
3. `dracula`;
4. `gruvbox` - Gruvbox Dark;
5. `light` - for bright terminal environments;
6. `high-contrast` - maximum foreground/background separation.

User-authored color palettes and per-role RGB configuration are outside the first
implementation. Additional built-in Themes may be added later without changing Theme
semantics or keyboard behavior.

`--theme <name>` overrides the saved Theme for the current process without changing the
saved preference. An unknown or unavailable Theme name falls back to `sessionmesh` instead
of preventing startup.

Theme changes affect presentation only. They never change thresholds, severity, ordering,
Session state, or Provider identity. Provider identity remains a text label and does not
own the alert palette. Required state and severity labels remain visible so color is never
the only evidence.

### UI-006 - Dense operational layout

**Status: Confirmed**

- Prefer rows, tables, lanes, and inspectors over a card-heavy dashboard.
- Use indentation, compact headings, and aligned columns for the
  `Project -> Worktree -> Session` hierarchy. Borders frame major regions such as `NOW`,
  `RECENT SIGNALS`, and Session Detail, not each Project, Worktree, or Session.
- Keep headings compact and optimized for scanning.
- Do not hide required evidence behind color alone.
- Truncate long project/model/session values predictably and expose full values in detail.
- Narrow terminals may collapse secondary fields, but must preserve attention kind,
  project, severity, and primary action.

### UI-012 - Responsive terminal bounds

**Status: Confirmed**

Use `120x30` as the full-layout target and `80x24` as the hard minimum. At the full
target, `NOW` and `RECENT SIGNALS` may render side by side. The
compact layout stacks them and degrades fields in this order: shorten Worktree path, hide
model, hide last activity, replace the Context bar with its percentage, then compress
Recent Signal evidence to one line.

Attention kind and severity, Project and Worktree identity, Session State, Provider,
Current Context percentage, selection, and Inspect remain available in the compact layout.
Below the hard minimum, render a terminal-too-small state with the required and current
dimensions plus Quit; do not attempt the normal layout.

The bounds are locked by deterministic production Ratatui renders under
`tests/fixtures/production-renders/`, asserted through the public production UI in
`tests/production_renders.rs`. Both sizes preserve the required hierarchy, state, Context
percentage, selection, and primary action without overlap; compact Detail uses vertical
scrolling only for lower-priority Compaction rows after identity, Usage, and complete
wrapped source metadata remain available.

### UI-008 - Context progress bar

**Status: Confirmed**

Each Session row shows Current Context occupancy as a stable-width progress bar plus a
text percentage. The bar represents `ContextState.tokens / ContextState.window`; it must
never represent cumulative processed Usage.

Theme roles for Open Sessions follow the Context thresholds:

- below 70% - Context normal;
- 70% through 84% - Context Warning;
- 85% and above - Context Critical.

The default `sessionmesh` Theme maps these roles to green, amber/yellow, and red. Other
Themes may use different accessible colors without changing the thresholds or labels.

The percentage remains visible so color is not the only signal. When the context window
is unknown, the UI renders a neutral/gray `N/A` state and does not invent a bar ratio. An
Exited session retains its last known percentage but uses neutral/gray presentation so
historical context does not look like a current alert.

On terminals too narrow for the stable-width bar, the Context column may degrade to the
text percentage or `N/A` while preserving the same threshold semantics.

## 12. Keyboard model

**Status: Confirmed**

Global commands:

- `1/2/3` - All, Claude Code, Codex CLI;
- `Tab` / `Shift+Tab` - move forward/backward among focusable regions on the current page;
  they never change the `1/2/3` page. On the All page they switch between `NOW` and
  `RECENT SIGNALS`; a page with only one focusable region leaves them as no-ops;
- `?` - reopen the Help overlay;
- `t` - cycle built-in Themes and save the selected Theme for the local Profile;
- `r` - refresh;
- `q` - quit;
- `Esc` - return from Session Detail or close the current inspector/Help layer.

List and timeline commands:

- `Up/Down` or `j/k` - move selection;
- `Enter` - activate the selected item's single primary action: inspect a Session or
  locate the Worktree belonging to a Shared Worktree Risk Signal.

Session Detail commands:

- `Up/Down` or `j/k` - scroll vertically;
- `c` - open the source-metadata Copy menu;
- `1/2/3` - leave Detail for a top-level page;
- `Esc` - return to the originating page and restore its view state.

Copy menu commands:

- `Up/Down` or `j/k` - select an available metadata field;
- `Enter` - copy its complete value to the macOS clipboard;
- `Esc` - close the menu without copying.

## 13. Privacy and action boundaries

The existing privacy contract remains non-negotiable:

- metadata only;
- no prompt, tool, response, or file-content storage;
- source logs remain the source of truth for provider session metadata;
- local processing only;
- provider sessions remain distinct even when grouped by project.

SessionMesh remains read-only. Labels such as `Resume`, `Stop`, or `Start fresh` must not
imply that SessionMesh performs those actions. The primary action is `Inspect` unless a
separate, explicitly approved command-launching capability is specified later.

## 14. Technical capability gaps

These are product dependencies, not an implementation plan:

1. **Project and Worktree identity** - current `cwd` metadata must be resolved to a
   canonical Git common directory, Worktree root, and branch without reading source
   contents. Non-Git directories use canonical `cwd` as standalone Project identity.
2. **State transitions** - current snapshots show latest state; Recent Signals require
   de-duplicated transition events or equivalent temporal metadata.
3. **Detail navigation** - `NOW`, `RECENT SIGNALS`, and Provider pages need a stable
   Provider-plus-native-Session-ID route into the same full-page Detail, plus restoration
   of the originating view state on return.
4. **Clipboard integration** - Session Detail needs an explicit user-triggered macOS
   clipboard write for selected source metadata, including status feedback on failure.
5. **Run boundaries** - Current Run Usage needs a provider-native live invocation
   boundary. Claude exposes `startedAt` for its live process; Codex requires the start
   time of the live process holding the rollout file. Completed Run boundaries are not
   retained, and Exited Sessions expose only Transcript Lifetime Usage.
### 14.1 Deferred capability

`Possible Stall` detection is Deferred from the first attention-first implementation. It
must not create an Attention Condition, Recent Signal, future Signal History event, or
notification until provider-specific evidence can distinguish a likely stall from
legitimate Busy work with an acceptable false-positive rate.

`Acknowledge` and `Snooze` are Deferred to a later requirement. The first implementation
does not provide their commands, key bindings, persistence, expiry, hiding, or ordering
behavior; current Attention Conditions remain derived entirely from observable state.

Full Signal History is Deferred to a later requirement. The first implementation provides
only the bounded `RECENT SIGNALS` list and does not include a History page or overlay,
global/Project/Session filters, search, a timeline inspector, or long-term event retention.
Recent Signals exist only for the current SessionMesh process and require no persistent event
store.

## 15. Product success criteria

The direction is successful when:

- a user with several Open Sessions can identify the next Session to inspect immediately;
- cross-provider worktree risk is visible without manually comparing provider tabs;
- users can understand an alert from its displayed evidence;
- returning users can reconstruct the important recent metadata changes retained in
  `RECENT SIGNALS`;
- the interface remains useful with zero Attention Conditions and does not manufacture
  urgency;
- all behavior preserves the local, read-only, metadata-only contract.

No remote analytics are required. Evaluation may use scripted scenarios, render fixtures,
and direct usability observation.

## 16. Open questions

| ID | Question | Status |
|---|---|---|
| Q-007 | Do the provisional `80x24` minimum and `120x30` full target work in the running TUI without overlap or unreadable truncation? | Confirmed |

## 17. Decision log

| Date | Decision | Status | Rationale |
|---|---|---|---|
| 2026-07-30 | Maintain this as a living local PRD alongside the v1 spec. | Confirmed | New requirement points will be reviewed and incorporated incrementally. |
| 2026-07-30 | Use exactly `1/2/3` as persistent top-level navigation for All, Claude Code, and Codex CLI. | Confirmed | Keeps the terminal navigation compact and easy to operate by keyboard. |
| 2026-07-30 | Omit text descriptions beside the persistent `1/2/3` navigation. | Confirmed | The mapping is introduced through Help instead of repeated in the chrome. |
| 2026-07-30 | Introduce the numeric mapping and essential keys in a small startup Help overlay. | Confirmed | Preserves discoverability without permanent explanatory text. |
| 2026-07-30 | Defer full Signal History and keep only `RECENT SIGNALS` in the first implementation. | Deferred | A bounded recent-change list covers the immediate return-to-terminal job without adding another view, filters, search, and long-term event retention. |
| 2026-07-30 | Do not use a separate Attention Inbox as the default-screen structure. | Rejected | The main screen uses one non-duplicated current Session inventory instead. |
| 2026-07-30 | Keep SessionMesh read-only and use `Inspect` as the primary action. | Confirmed | Preserves the existing v1 trust boundary. |
| 2026-07-30 | Use Theme-defined semantic colors, with severity and state rather than Provider owning alert meaning. | Confirmed | A default Theme gives SessionMesh a coherent identity while runtime cycling, Profile persistence, and accessible alternatives can change presentation without changing behavior. |
| 2026-07-30 | Ship six built-in Themes and defer user-authored palettes. | Confirmed | SessionMesh, Catppuccin, Dracula, Gruvbox, Light, and High Contrast cover mainstream dark, bright-terminal, and accessibility needs without expanding first-version configuration and validation across a large palette catalog. |
| 2026-07-30 | Show complete working-directory and source-log metadata in Session Detail without exposing raw contents. | Confirmed | Local paths and native identity support investigation, while raw JSONL, prompt/response, tool, and source-code content remain outside the privacy boundary. |
| 2026-07-30 | Group provider-page sessions by Project while retaining one row per Session. | Confirmed | Project supplies context without losing session-specific state or detail navigation. |
| 2026-07-30 | Render Current Context as a percentage progress bar with threshold colors. | Confirmed | Makes context pressure scannable while retaining precise text and unknown-state handling. |
| 2026-07-30 | Divide the main terminal screen into regions, with `NOW` and `RECENT SIGNALS` as the only assigned regions. | Confirmed | The homepage should combine current state and recent changes without letting one Session list occupy the entire terminal. |
| 2026-07-30 | Leave the other two proposed homepage regions unassigned. | Confirmed | Avoids prematurely placing additional focus or Signal History behavior before their user jobs are settled. |
| 2026-07-30 | Show Current Run Usage separately from Transcript Lifetime Usage in Session detail. | Confirmed | Current Run supports comparison with Claude Code `/usage`; transcript lifetime preserves inherited usage without presenting it as the same total. |
| 2026-07-30 | Use the provider Run boundary, not the latest model, to calculate Current Run Usage. | Confirmed | A single Run may use multiple models, while a resumed Session may retain earlier transcript usage. |
| 2026-07-30 | Show Current Run Usage only while a Session is Open; show only Transcript Lifetime Usage after exit. | Confirmed | JSONL timestamps preserve lifetime Usage but not a durable provider Run boundary. Avoiding inferred or persisted Last Run Usage keeps the index rebuildable and the lifecycle language precise. |
| 2026-07-30 | Expose same-ID JSONL copies as a Transcript Lineage in Session Detail. | Rejected | The observed duplicate files were manually copied and do not establish normal Provider behavior; Session Detail retains one Source Log. |
| 2026-07-30 | Name structural same-worktree exposure `Shared Worktree Risk`, not worktree conflict. | Confirmed | Metadata can prove that Open Sessions share one working copy, but cannot prove that they edit the same files or have produced a Git conflict. |
| 2026-07-30 | Treat Idle as a current state and Busy-to-Idle as a transition event. | Confirmed | The domain model already defines every Open Idle Session as awaiting input; only a state change belongs in Recent Signals. |
| 2026-07-30 | Define an Open Session by attachment to a validated live provider CLI process. | Confirmed | Busy and Idle describe a live process; source-log existence or recent activity alone does not prove that a Session remains open. |
| 2026-07-30 | Treat every Idle Open Session as neutral Attention, not an Alert. | Confirmed | Awaiting input may require user action, but it is neither a risk nor an abnormal condition and must not manufacture urgency. |
| 2026-07-30 | Defer `Possible Stall` detection from the first attention-first implementation. | Deferred | Busy state plus absent source-log growth cannot reliably distinguish a stall from legitimate work within the metadata-only boundary. |
| 2026-07-30 | Keep Exited Sessions out of current Attention and record only their transition. | Confirmed | Exited Sessions are not Open and therefore do not belong in `NOW`; the transition remains useful in Recent Signals. |
| 2026-07-30 | Give Shared Worktree Risk fixed Warning severity. | Confirmed | Shared-worktree evidence is important but does not prove an actual conflict; Session count strengthens the evidence display without justifying Critical severity. |
| 2026-07-30 | Identify a Git Project by canonical Git common directory and a Worktree by canonical top-level path. | Confirmed | Linked Worktrees must group under one local Project, while independent clones remain separate even when they share a remote URL. |
| 2026-07-30 | Rank current conditions as Critical Context, Shared Worktree Risk, Warning Context, Idle, then healthy Busy. | Confirmed | Critical outranks Warning; shared-worktree exposure has broader impact than an early Context warning; neutral Idle remains actionable without being presented as risk. |
| 2026-07-30 | Break equal Attention ranks with condition-specific evidence, recency, then stable identity. | Confirmed | The order stays deterministic and explainable without introducing an opaque weighted score; recently Idle work is more relevant than indefinitely pinning stale Idle Sessions. |
| 2026-07-30 | Integrate Project and Worktree organization into `NOW` instead of creating a separate Projects capability. | Confirmed | The main page already owns the cross-Provider Open Session inventory; a second view would duplicate the same Sessions and conflict with the fixed three-page navigation. |
| 2026-07-30 | Keep Processed Usage and Estimated Cost off the main screen. | Confirmed | `NOW` prioritizes current action and evidence; Current Context remains because it is operational state, while cumulative usage and cost stay in Provider pages and Session detail. |
| 2026-07-30 | Defer `Acknowledge` and `Snooze` to a later requirement. | Deferred | A factual current-state view does not yet need user-authored suppression state, expiry rules, persistence, or dedicated commands. |
| 2026-07-30 | Keep `RECENT SIGNALS` process-lifetime only. | Confirmed | Live state transitions cannot be reconstructed reliably after SessionMesh was closed; an initial snapshot is a baseline, not a set of newly observed events. |
| 2026-07-30 | Limit Recent Signals to actionable lifecycle, Context, Compaction, and Shared Worktree Risk changes. | Confirmed | Excluding Idle-to-Busy, heartbeats, ordinary metric changes, and unchanged polls keeps the bounded list focused; a Compaction and its resulting Alert clear are one compound Signal. |
| 2026-07-30 | Retain at most the five newest Recent Signals without scrolling or expansion. | Confirmed | Five changes provide short return-to-terminal context without recreating the Deferred Signal History capability; compact layouts yield space to `NOW` first. |
| 2026-07-30 | Use `80x24` as the hard minimum and `120x30` as the full-layout target. | Confirmed | Deterministic interactive Ratatui renders at both sizes preserved the required hierarchy, state, Context, selection, actions, and complete Detail metadata without overlap; `79x23` correctly falls back to the terminal-too-small state. |
| 2026-07-30 | Name the collapsed older provider inventory `OLDER SESSIONS`, not History. | Confirmed | It contains Session rows outside the primary recency scope, while Signal History means a chronological event capability and is Deferred. |
| 2026-07-30 | Organize Provider pages by recent-first Projects with a per-Project `OLDER SESSIONS` disclosure. | Confirmed | Project groups preserve work context while their latest child orders the groups; Recent and Older Sessions remain time-ordered inside each Project without creating a separate global archive. |
| 2026-07-30 | Auto-open Help once per local Profile and keep it available through `?`. | Confirmed | First-use discovery is necessary for numeric navigation, while showing the overlay on every process launch would interrupt repeat use. |
| 2026-07-30 | Render Session Detail as a full-page child view instead of a centered overlay. | Confirmed | Detail needs room for evidence, source metadata, scoped Usage, and Compactions; persistent numeric navigation and origin-state restoration keep it connected to the three top-level pages. |
| 2026-07-30 | Add a `c` Copy menu for source paths and native Session ID in Session Detail. | Confirmed | Explicit field selection copies complete metadata values without exposing raw JSONL or modifying Provider Sessions. |
| 2026-07-30 | Reserve `1/2/3` for top-level pages and use `Tab`/`Shift+Tab` for focus within the current page. | Confirmed | Numeric page navigation is already direct; local focus traversal makes the independent `NOW` and `RECENT SIGNALS` selections reachable without duplicating page navigation. |
| 2026-07-30 | Show condition-specific evidence directly in `NOW`, including a Context progress bar plus percentage. | Confirmed | Users should be able to trust Context, Shared Worktree Risk, Idle, and healthy Busy classifications before opening Detail; compact layouts may degrade the bar to percentage text. |
| 2026-07-30 | Use compact hierarchical rows and reserve borders for major regions. | Confirmed | Dense alignment keeps multiple concurrent Sessions comparable in one terminal viewport without turning Projects, Worktrees, and Sessions into nested cards. |
| 2026-07-30 | Keep Recent Signals out of Session Detail. | Confirmed | The main page owns the bounded process-lifetime change list; Detail owns current evidence and provider-reconstructable Usage, Compaction, and source metadata without introducing a second Signal History surface. |
| 2026-07-30 | Route Recent Signal actions by scope: Session Signals inspect Detail and Shared Worktree Risk Signals locate the Worktree in `NOW`. | Confirmed | A worktree-level condition may involve multiple Sessions, so selecting one arbitrarily would misrepresent the Signal; cleared, no-longer-open Worktrees report that navigation is unavailable. |

## 18. Adding the next requirement

Use the following shape when incorporating a new requirement point:

```text
ID:
Status: Proposed | Confirmed | Deferred | Rejected
Requirement:
User value:
Acceptance criteria:
Affected screens/data:
Open questions:
```

Do not append ideas as an unstructured backlog. Integrate each confirmed point into the
relevant workflow above and record the resulting decision in the log.
