# ADR 0006 - Separate successor launch plans from terminal adapters

There is no portable operating-system interface for opening a new terminal and running an
interactive command, so embedding AppleScript or a shell string in Handoff logic would tie
Task, Provider, and UI behavior to macOS. Mudi will instead produce a structured,
platform-neutral Successor Launch Plan and pass it through a Terminal Launcher module seam;
the v1 adapter opens macOS `Terminal.app`, while `Copy Command` remains the fallback and
future platforms or terminal applications add adapters without changing Handoff semantics.

The Plan carries a working directory, executable, argument list, and Artifact reference
rather than an interpolated command string. This rejects both pretending that one portable
terminal-launch command exists and implementing untestable Linux or Windows adapters before
the rest of Mudi supports those platforms.
