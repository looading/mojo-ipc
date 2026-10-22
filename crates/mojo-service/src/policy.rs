use std::collections::BTreeSet;

/// A logical service and its exact interface name, independent of any PID.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ServiceKey {
    pub name: String,
    pub interface_name: String,
}

impl ServiceKey {
    pub fn new(name: impl Into<String>, interface_name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            interface_name: interface_name.into(),
        }
    }

    pub(crate) fn is_valid(&self) -> bool {
        [&self.name, &self.interface_name]
            .into_iter()
            .all(|s| !s.is_empty() && s.len() <= 256 && !s.chars().any(char::is_control))
    }
}

/// Host-assigned, default-deny permissions. Grants match complete service keys.
#[derive(Clone, Debug, Default)]
pub struct AccessPolicy {
    publish: BTreeSet<ServiceKey>,
    connect: BTreeSet<ServiceKey>,
}

impl AccessPolicy {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn allow_publish(mut self, key: ServiceKey) -> Self {
        self.publish.insert(key);
        self
    }

    pub fn allow_connect(mut self, key: ServiceKey) -> Self {
        self.connect.insert(key);
        self
    }

    pub fn can_publish(&self, key: &ServiceKey) -> bool {
        self.publish.contains(key)
    }

    pub fn can_connect(&self, key: &ServiceKey) -> bool {
        self.connect.contains(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grants_do_not_authorize_other_interfaces_or_operations() {
        let key = ServiceKey::new("files", "demo.Files");
        let other = ServiceKey::new("files", "demo.Admin");
        let policy = AccessPolicy::new().allow_connect(key.clone());
        assert!(policy.can_connect(&key));
        assert!(!policy.can_publish(&key));
        assert!(!policy.can_connect(&other));
        assert!(!AccessPolicy::new().can_connect(&key));
    }

    #[test]
    fn rejects_empty_control_and_oversized_names() {
        for name in ["".to_owned(), "bad\0name".to_owned(), "x".repeat(257)] {
            assert!(!ServiceKey::new(name, "demo.Echo").is_valid());
        }
        assert!(ServiceKey::new("应用", "demo.Echo").is_valid());
    }
}
