use super::*;

#[test]
fn role_uses_the_configured_warning_and_critical_thresholds() {
    assert_eq!(role(WARN_PCT - 0.1), Role::Normal);
    assert_eq!(role(WARN_PCT), Role::Warning);
    assert_eq!(role(CRIT_PCT), Role::Critical);
}
