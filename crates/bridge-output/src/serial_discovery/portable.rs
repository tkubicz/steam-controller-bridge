pub(super) fn is_callout_port(_path: &str) -> bool {
    true
}

#[cfg(test)]
pub(super) const TEST_CALLOUT_PORT: &str = "serial:test";
