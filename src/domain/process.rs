/// Exact operating-system process evidence used to defend control actions from PID reuse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessIdentity {
    pub pid: u32,
    pub started_at_ms: i64,
}
