# Attention UI prototype verdict

## Question

Does the confirmed attention-first layout work at `120x30` and `80x24` without overlap,
incoherent truncation, or loss of required actions?

## Verdict

Yes. Q-007 and UI-012 are Confirmed in `docs/attention-first-ui-prd.md`.

At `120x30`, `NOW` and `RECENT SIGNALS` work side by side. At `80x24`, they stack;
`NOW` retains enough height for the complete five-Session fixture while Recent Signals
degrade from five multi-line entries to four one-line entries. The compact rows preserve
Project and shortened Worktree identity, attention/severity, state, Provider, Context
percentage or `N/A`, selection, and the primary action.

Provider pages, Open and Exited Detail, expanded `OLDER SESSIONS`, and the Copy menu fit
at both targets. Open Detail keeps Current Run Usage distinct from Transcript Lifetime
Usage; Exited Detail never renders Current Run Usage. Complete paths wrap in Detail.
Compaction rows may require vertical scrolling at `80x24`. A `79x23` terminal renders the
required too-small state instead of the normal layout.

The `tokenplay`, `light`, and `high-contrast` ANSI fixtures preserve distinct selected,
muted, Warning, Critical, Idle, and healthy Busy styles. The fixture generator checks
minimum contrast ratios and the required navigation/state-restoration interactions.

## Review

Run the interactive prototype:

```sh
cargo run --example attention_ui_prototype
```

Regenerate all fixed-size plain-text and ANSI fixtures:

```sh
cargo run --example attention_ui_prototype -- --render-fixtures
```

The reviewable outputs are indexed in `docs/prototype-attention/renders/README.md`.
Use `less -R <file>.ansi` to retain Theme colors.

## Disposal

`examples/attention_ui_prototype.rs` and `docs/prototype-attention/` are throwaway
artifacts. Keep them through issue planning and implementation review, then delete them
after the validated responsive rules have been absorbed into production code and tests.
The PRD decision is the durable answer.
