# ADR 0004 - End an Open Current Run safely before Handoff

Resuming a Session while its manually started Current Run is still open risks concurrent
transcript writes, but requiring every user to exit it manually makes assisted Handoff
needlessly fragile. A Handoff may therefore include an explicitly disclosed choice to end
the Current Run: SessionMesh waits until the Run is Idle, revalidates the provider process,
requests termination without killing its Terminal or shell, and proceeds only after exit is
observed. It never terminates a Busy Run, targets an unverified PID, or escalates
automatically to a force kill. A recoverable failure remains Waiting only within the
declared two-hour authorization deadline; if no control action has occurred when that
deadline expires, the Operation Task becomes Cancelled and requires new authorization.
