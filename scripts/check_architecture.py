#!/usr/bin/env python3
import argparse
import re
import sys
from dataclasses import dataclass
from pathlib import Path


@dataclass(frozen=True, order=True)
class Violation:
    path: Path
    line: int
    rule: str
    detail: str


def layer_dependency_pattern(*layers):
    names = "|".join(layers)
    return re.compile(
        rf"\b(?:crate|super)\s*::\s*(?:{names})\b"
        rf"|\b(?:crate|super)\s*::\s*\{{[^}}]*\b(?:{names})\b"
        rf"|\b(?:{names})\s*::",
        re.DOTALL,
    )


PROCESS_COMMAND = re.compile(
    r"\bstd\s*::\s*process\s*::\s*(?:\{[^}]*\bCommand\b|Command\b)"
    r"|\bprocess\s*::\s*Command\b|\bCommand\s*::\s*new\b"
    r"|\b(?:std\s*::\s*)?process\s+as\s+\w+\b",
    re.DOTALL,
)

DOMAIN_PATTERNS = (
    (
        layer_dependency_pattern("application", "adapters", "presentation"),
        "outer-layer dependency",
    ),
    (re.compile(r"\b(?:rusqlite|ratatui)\b"), "concrete database or UI dependency"),
    (
        re.compile(r"\b(?:std|tokio)\s*::\s*process\b|\bCommand\s*::\s*new\b"),
        "process implementation",
    ),
    (
        re.compile(
            r"\b(?:std|tokio)\s*::\s*(?:(?:fs|net)\b|\{[^}]*\b(?:fs|net)\b)",
            re.DOTALL,
        ),
        "filesystem or network implementation",
    ),
    (re.compile(r"\b(?:reqwest|ureq|walkdir|dirs)\s*::"), "filesystem or network implementation"),
)

APPLICATION_PATTERNS = (
    (layer_dependency_pattern("adapters", "presentation"), "outer or concrete dependency"),
    (re.compile(r"\b(?:rusqlite|ratatui)\b"), "concrete database or UI dependency"),
    (PROCESS_COMMAND, "direct process command construction"),
)

PRESENTATION_PATTERNS = (
    (layer_dependency_pattern("adapters"), "concrete adapter dependency"),
    (
        re.compile(
            r"\b(?:[A-Z][A-Za-z0-9_]*Repository|StateDatabase|TaskStore|SessionLabels)\b"
        ),
        "authoritative repository dependency",
    ),
    (re.compile(r"\b(?:rusqlite|SQLite)\b"), "SQLite dependency"),
    (
        re.compile(
            r"\b(?:SessionObservationSource|ManagedBindingEvidenceSource|ProviderSourceReader)\b"
        ),
        "Provider source reader dependency",
    ),
    (
        re.compile(r"\b(?:ProcessEvidence|SystemProcessEvidence|ProcessProbe)\b"),
        "process probe dependency",
    ),
    (PROCESS_COMMAND, "direct process command construction"),
    (
        re.compile(
            r"\b(?:Tmux[A-Za-z0-9_]*|MacosManagedTerminal[A-Za-z0-9_]*|"
            r"MacosNotification|MacosClipboard|ProviderExecutables)\b"
        ),
        "concrete system adapter dependency",
    ),
    (re.compile(r"\bstate_database_path\b|state\.db"), "authoritative state path"),
)

STATE_SCHEMA_OWNER = Path("src/adapters/persistence/state/mod.rs")
DISPOSABLE_INDEX = Path("src/adapters/persistence/index")
SCHEMA_DEFINITION = re.compile(
    r"\bCREATE\s+(?:(?:UNIQUE|TEMP|TEMPORARY)\s+)*"
    r"(?:VIRTUAL\s+TABLE|TABLE|INDEX|VIEW|TRIGGER)\b",
    re.IGNORECASE,
)
USER_VERSION = re.compile(r"\buser_version\b")
OPPORTUNISTIC_MIGRATIONS = (
    (re.compile(r"\bPRAGMA\s+table_info\b", re.IGNORECASE), "PRAGMA table_info probe"),
    (re.compile(r"\bALTER\s+TABLE\b", re.IGNORECASE), "ALTER TABLE migration"),
)
AUTHORITATIVE_WRITE_PATTERNS = (
    (
        re.compile(r"\b[A-Z][A-Za-z0-9_]*Repository\b"),
        "authoritative write repository interface",
    ),
    (
        re.compile(
            r"\.\s*(?:insert_resume|record_launch_title_association|associate_launch_title|"
            r"establish_management|record_launched|record_terminal_opened|record_terminal_error|"
            r"remove_binding|mark_needs_review|mark_failed|hide_sessions|remove_from_hide|"
            r"permanently_exclude_sessions|set_alias|associate_alias|clear_title|set_description|"
            r"set_metadata|request_cancellation|mark_termination_requested|mark_cancelled|"
            r"mark_retry_ready|mark_ready|request_deletion|record_deletion_failure|"
            r"delete_terminal|mark_interrupted|cancel_before_execution|acknowledge)\s*\("
        ),
        "authoritative repository write call",
    ),
)
CONCRETE_IMPLEMENTATION_PATTERNS = (
    (re.compile(r"\brusqlite\b"), "SQLite implementation outside adapters"),
    (PROCESS_COMMAND, "process command implementation outside adapters"),
    (
        re.compile(r"(?:/usr/bin/)?\b(?:osascript|pbcopy)\b"),
        "macOS command implementation outside adapters",
    ),
    (
        re.compile(
            r"\b(?:struct|enum)\s+(?:Tmux[A-Za-z0-9_]*|MacosManagedTerminal[A-Za-z0-9_]*|"
            r"MacosNotification|MacosClipboard|SystemProcessEvidence)\b"
        ),
        "concrete external implementation outside adapters",
    ),
    (
        re.compile(
            r"\bfn\s+(?:inspect_process_identity|process_identity|process_identity_after_spawn|"
            r"process_identity_is_live|discover_provider_process)\b"
        ),
        "Provider process implementation outside adapters",
    ),
)


def parse_args():
    parser = argparse.ArgumentParser(description="Check Mudi's production architecture boundaries")
    parser.add_argument(
        "--root",
        type=Path,
        default=Path(__file__).resolve().parents[1],
        help="repository root (defaults to the parent of scripts/)",
    )
    return parser.parse_args()


def rust_files(root, directory):
    directory = root / directory
    if not directory.is_dir():
        return []
    return [
        path
        for path in sorted(directory.rglob("*.rs"))
        if not path.name.endswith("_test.rs")
        and not {"tests", "fixtures"}.intersection(path.relative_to(root).parts)
    ]


def matching_lines(path, pattern):
    source = path.read_text(encoding="utf-8")
    for match in pattern.finditer(source):
        yield source.count("\n", 0, match.start()) + 1


def check_patterns(root, directory, rule, patterns):
    violations = []
    for path in rust_files(root, directory):
        for pattern, detail in patterns:
            for line in matching_lines(path, pattern):
                violations.append(
                    Violation(
                        path.relative_to(root),
                        line,
                        rule,
                        detail,
                    )
                )
    return violations


def check_domain(root):
    return check_patterns(
        root,
        "src/domain",
        "ARCH001 domain-dependencies",
        DOMAIN_PATTERNS,
    )


def check_application(root):
    return check_patterns(
        root,
        "src/application",
        "ARCH002 application-dependencies",
        APPLICATION_PATTERNS,
    )


def check_presentation(root):
    return check_patterns(
        root,
        "src/presentation",
        "ARCH003 presentation-ownership",
        PRESENTATION_PATTERNS,
    )


def first_matching_line(source, pattern):
    match = pattern.search(source)
    if match is None:
        return None
    return source.count("\n", 0, match.start()) + 1


def check_authoritative_schema_owner(root):
    rule = "ARCH004 authoritative-schema-owner"
    owner = root / STATE_SCHEMA_OWNER
    violations = []
    if not owner.is_file():
        return [Violation(STATE_SCHEMA_OWNER, 1, rule, "declared State Schema Owner is missing")]

    owner_source = owner.read_text(encoding="utf-8")
    required_markers = (
        (re.compile(r"\bstruct\s+StateDatabase\b"), "StateDatabase declaration"),
        (SCHEMA_DEFINITION, "authoritative schema definition"),
        (USER_VERSION, "PRAGMA user_version ownership"),
    )
    for pattern, description in required_markers:
        if pattern.search(owner_source) is None:
            violations.append(
                Violation(
                    STATE_SCHEMA_OWNER,
                    1,
                    rule,
                    f"declared owner is missing {description}",
                )
            )

    for path in rust_files(root, "src"):
        relative = path.relative_to(root)
        if relative == STATE_SCHEMA_OWNER or relative.parts[:4] == DISPOSABLE_INDEX.parts:
            continue
        source = path.read_text(encoding="utf-8")
        for pattern, description in (
            (SCHEMA_DEFINITION, "schema creation outside the declared owner"),
            (USER_VERSION, "user_version access outside the declared owner"),
        ):
            line = first_matching_line(source, pattern)
            if line is not None:
                violations.append(Violation(relative, line, rule, description))
    return violations


def check_opportunistic_migrations(root):
    rule = "ARCH005 no-opportunistic-migrations"
    violations = []
    for path in rust_files(root, "src/adapters/persistence"):
        relative = path.relative_to(root)
        if relative == STATE_SCHEMA_OWNER or relative.parts[:4] == DISPOSABLE_INDEX.parts:
            continue
        for pattern, description in OPPORTUNISTIC_MIGRATIONS:
            for line in matching_lines(path, pattern):
                violations.append(Violation(relative, line, rule, description))
    return violations


def observation_stage_files(root, stage):
    files = []
    single_file = root / f"src/application/observation/{stage}.rs"
    if single_file.is_file():
        files.append(single_file)
    files.extend(rust_files(root, f"src/application/observation/{stage}"))
    return files


def check_observation_read_only(root):
    rule = "ARCH006 observation-read-only"
    violations = []
    for stage in ("collect", "project"):
        for path in observation_stage_files(root, stage):
            for pattern, description in AUTHORITATIVE_WRITE_PATTERNS:
                for line in matching_lines(path, pattern):
                    violations.append(
                        Violation(path.relative_to(root), line, rule, description)
                    )
    return violations


def check_concrete_adapter_placement(root):
    rule = "ARCH007 concrete-adapter-placement"
    violations = []
    for path in rust_files(root, "src"):
        relative = path.relative_to(root)
        if relative.parts[:2] == ("src", "adapters"):
            continue
        for pattern, description in CONCRETE_IMPLEMENTATION_PATTERNS:
            for line in matching_lines(path, pattern):
                violations.append(Violation(relative, line, rule, description))
    return violations


def main():
    args = parse_args()
    root = args.root.resolve()
    violations = (
        check_domain(root)
        + check_application(root)
        + check_presentation(root)
        + check_authoritative_schema_owner(root)
        + check_opportunistic_migrations(root)
        + check_observation_read_only(root)
        + check_concrete_adapter_placement(root)
    )
    if violations:
        for violation in sorted(set(violations)):
            print(
                f"{violation.rule}: {violation.path}:{violation.line}: {violation.detail}",
                file=sys.stderr,
            )
        return 1
    print("Architecture check passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
