import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


CHECKER = Path(__file__).with_name("check_architecture.py")


class ArchitectureCheckerContractTests(unittest.TestCase):
    def setUp(self):
        self.temporary_directory = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary_directory.name)
        self.addCleanup(self.temporary_directory.cleanup)
        self.write_allowed_repository()

    def write(self, relative_path, source):
        path = self.root / relative_path
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(source, encoding="utf-8")

    def write_allowed_repository(self):
        self.write("src/domain/mod.rs", "pub struct Session;\n")
        self.write("src/application/mod.rs", "use crate::domain::Session;\n")
        self.write("src/application/observation/collect.rs", "pub struct CollectService;\n")
        self.write("src/application/observation/project.rs", "pub struct ProjectionService;\n")
        self.write("src/presentation/mod.rs", "use crate::application::App;\n")
        self.write("src/adapters/mod.rs", "pub struct Adapter;\n")
        self.write(
            "src/adapters/persistence/state/mod.rs",
            """pub const STATE_SCHEMA_VERSION: i64 = 1;
pub struct StateDatabase;
const STATE_SCHEMA: &str = r#"CREATE TABLE records (id TEXT PRIMARY KEY);"#;
fn initialize_schema(connection: &Connection) {
    connection.pragma_update(None, "user_version", STATE_SCHEMA_VERSION);
}
""",
        )

    def run_checker(self):
        return subprocess.run(
            [sys.executable, str(CHECKER), "--root", str(self.root)],
            check=False,
            capture_output=True,
            text=True,
        )

    def assert_allowed(self):
        result = self.run_checker()
        self.assertEqual(result.returncode, 0, result.stderr)

    def assert_violation(self, rule, path):
        result = self.run_checker()
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertIn(rule, result.stderr)
        self.assertIn(path, result.stderr)

    def test_domain_dependencies_allow_domain_rules_and_reject_outer_or_io_implementations(self):
        self.assert_allowed()

        prohibited = {
            "crate::application": "use crate::application::Workflow;\n",
            "crate::adapters": "use crate::adapters::Adapter;\n",
            "grouped crate adapter": "use crate::{adapters::Adapter, domain::Session};\n",
            "aliased crate adapter": "use crate::adapters as outer;\n",
            "crate::presentation": "use crate::presentation::Ui;\n",
            "SQLite": "use rusqlite::Connection;\n",
            "process commands": "use std::process::Command;\n",
            "Ratatui": "use ratatui::Frame;\n",
            "filesystem": "use std::fs;\n",
            "network": "use std::net::TcpStream;\n",
        }
        for name, source in prohibited.items():
            with self.subTest(name=name):
                self.write("src/domain/mod.rs", source)
                self.assert_violation("ARCH001 domain-dependencies", "src/domain/mod.rs:1")

    def test_application_dependencies_allow_ports_and_reject_outer_or_concrete_dependencies(self):
        self.assert_allowed()

        prohibited = {
            "crate::adapters": "use crate::adapters::persistence::Store;\n",
            "grouped crate adapter": "use crate::{adapters::Store, domain::Session};\n",
            "aliased crate adapter": "use crate::adapters as infrastructure;\n",
            "crate::presentation": "use crate::presentation::Ui;\n",
            "SQLite": "use rusqlite::Connection;\n",
            "Ratatui": "use ratatui::Frame;\n",
            "process import": "use std::process::Command;\n",
            "process construction": "fn run() { std::process::Command::new(\"tool\"); }\n",
            "aliased process module": (
                "use std::process as operating_system;\n"
                "fn run() { operating_system::Command::new(\"tool\"); }\n"
            ),
        }
        for name, source in prohibited.items():
            with self.subTest(name=name):
                self.write("src/application/mod.rs", source)
                self.assert_violation(
                    "ARCH002 application-dependencies", "src/application/mod.rs:1"
                )

    def test_presentation_allows_commands_and_wording_but_rejects_state_or_system_ownership(self):
        self.write(
            "src/presentation/mod.rs",
            """use crate::application::clipboard::ClipboardPort;
fn labels() -> (&'static str, &'static str) {
    ("Open in Terminal.app", "private tmux")
}
""",
        )
        self.assert_allowed()

        prohibited = {
            "concrete adapter": "use crate::adapters::persistence::TaskStore;\n",
            "aliased concrete adapter": "use crate::adapters as infrastructure;\n",
            "repository port": "use crate::application::tasks::TaskRepository;\n",
            "State Database": "fn open(database: StateDatabase) {}\n",
            "SQLite": "use rusqlite::Connection;\n",
            "Provider source": "fn collect(source: SessionObservationSource) {}\n",
            "process probe": "fn probe(evidence: SystemProcessEvidence) {}\n",
            "process command": "fn run() { std::process::Command::new(\"ps\"); }\n",
            "tmux adapter": "fn launch(broker: TmuxLaunchBroker) {}\n",
            "Terminal adapter": "fn open(terminal: MacosManagedTerminalAccess) {}\n",
            "notification adapter": "fn notify(adapter: MacosNotification) {}\n",
            "clipboard adapter": "fn copy(adapter: MacosClipboard) {}\n",
            "Provider process adapter": "fn runtime(value: ProviderExecutables) {}\n",
            "authoritative path": "const STATE_PATH: &str = \"state.db\";\n",
        }
        for name, source in prohibited.items():
            with self.subTest(name=name):
                self.write("src/presentation/mod.rs", source)
                self.assert_violation(
                    "ARCH003 presentation-ownership", "src/presentation/mod.rs:1"
                )

    def test_authoritative_schema_has_exactly_one_declared_owner(self):
        self.assert_allowed()

        self.write(
            "src/adapters/persistence/tasks.rs",
            """const TASK_SCHEMA: &str = "CREATE TABLE tasks (id TEXT);";
fn version(connection: &Connection) {
    connection.pragma_update(None, "user_version", 2);
}
""",
        )
        self.assert_violation(
            "ARCH004 authoritative-schema-owner",
            "src/adapters/persistence/tasks.rs:1",
        )

    def test_authoritative_schema_owner_must_contain_schema_and_version_contract(self):
        self.write(
            "src/adapters/persistence/state/mod.rs",
            "pub struct StateDatabase;\n",
        )
        self.assert_violation(
            "ARCH004 authoritative-schema-owner",
            "src/adapters/persistence/state/mod.rs:1",
        )

    def test_repository_open_paths_reject_opportunistic_schema_migration(self):
        self.write(
            "src/adapters/persistence/index/mod.rs",
            """const INDEX_PROBE: &str = "PRAGMA table_info(sessions)";
const INDEX_RESET: &str = "ALTER TABLE sessions DROP COLUMN legacy";
""",
        )
        self.assert_allowed()

        prohibited = {
            "table probe": 'const PROBE: &str = "PRAGMA table_info(tasks)";\n',
            "ordinary ALTER": 'const MIGRATION: &str = "ALTER TABLE tasks ADD COLUMN title";\n',
        }
        for name, source in prohibited.items():
            with self.subTest(name=name):
                self.write("src/adapters/persistence/tasks.rs", source)
                self.assert_violation(
                    "ARCH005 no-opportunistic-migrations",
                    "src/adapters/persistence/tasks.rs:1",
                )

    def test_collect_and_project_accept_queries_but_reject_authoritative_writes(self):
        self.write(
            "src/application/observation/collect.rs",
            "trait ManagedObservationQuery { fn registrations(&self); }\n",
        )
        self.write(
            "src/application/observation/project.rs",
            "trait OperationTaskQuery { fn tasks(&self); }\n",
        )
        self.assert_allowed()

        prohibited = {
            "src/application/observation/collect.rs": (
                "fn collect(repo: &dyn ManagedSessionRepository) { "
                "repo.record_terminal_opened(\"registration\", 1); }\n"
            ),
            "src/application/observation/project.rs": (
                "fn project(repo: &mut dyn SessionMetadataRepository) { "
                "repo.set_alias(Provider::CodexCli, \"session\", \"Title\"); }\n"
            ),
        }
        for path, source in prohibited.items():
            with self.subTest(path=path):
                self.write(path, source)
                self.assert_violation(
                    "ARCH006 observation-read-only", f"{path}:1"
                )

    def test_concrete_external_implementations_are_confined_to_adapters(self):
        self.write(
            "src/adapters/system/example.rs",
            """use rusqlite::Connection;
use std::process::Command;
struct TmuxExample;
const CLIPBOARD: &str = "/usr/bin/pbcopy";
""",
        )
        self.assert_allowed()

        prohibited = {
            "SQLite": "use rusqlite::Connection;\n",
            "process command": "use std::process::Command;\n",
            "tmux implementation": "struct TmuxImplementation;\n",
            "Terminal.app implementation": 'const TOOL: &str = "/usr/bin/osascript";\n',
            "notification implementation": "struct MacosNotification;\n",
            "clipboard implementation": 'const TOOL: &str = "/usr/bin/pbcopy";\n',
            "Provider process implementation": (
                "fn inspect_process_identity(pid: u32) -> bool { pid > 0 }\n"
            ),
        }
        for name, source in prohibited.items():
            with self.subTest(name=name):
                self.write("src/main.rs", source)
                self.assert_violation(
                    "ARCH007 concrete-adapter-placement", "src/main.rs:1"
                )


if __name__ == "__main__":
    unittest.main()
