use crate::{
    AccessPolicy, ServiceChangeKind, ServiceInfo, ServiceKey, Status,
    watch::{WatchBuffer, clone_info},
};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Instant;

pub(crate) struct Entry<F> {
    pub owner: u64,
    pub info: ServiceInfo,
    pub factory: F,
    registered_at: Instant,
}

pub(crate) struct ProbeEntry<F> {
    pub id: u64,
    pub remote: F,
    registered_at: Instant,
}

pub(crate) struct WatchEntry {
    pub id: u64,
    pub buffer: Arc<WatchBuffer>,
    policy: AccessPolicy,
}

pub(crate) struct WatchInstall {
    pub id: u64,
    pub snapshot: Vec<ServiceInfo>,
    pub previous: Option<WatchEntry>,
}

/// Kept independent of native endpoints so ownership/race rules are testable.
pub(crate) struct Registry<F> {
    entries: BTreeMap<ServiceKey, Entry<F>>,
    sessions: BTreeSet<u64>,
    watches: BTreeMap<u64, WatchEntry>,
    probes: BTreeMap<u64, ProbeEntry<F>>,
    next_id: u64,
}

impl<F> Default for Registry<F> {
    fn default() -> Self {
        Self {
            entries: BTreeMap::new(),
            sessions: BTreeSet::new(),
            watches: BTreeMap::new(),
            probes: BTreeMap::new(),
            next_id: 1,
        }
    }
}

impl<F> Registry<F> {
    fn allocate_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id = id
            .checked_add(1)
            .expect("service identifier space exhausted");
        id
    }

    pub fn open_session(&mut self) -> u64 {
        let id = self.allocate_id();
        self.sessions.insert(id);
        id
    }

    pub fn is_open(&self, session: u64) -> bool {
        self.sessions.contains(&session)
    }

    pub fn install_probe(
        &mut self,
        owner: u64,
        remote: F,
    ) -> Result<(u64, Option<ProbeEntry<F>>), (Status, F)> {
        if !self.is_open(owner) {
            return Err((Status::Unavailable, remote));
        }
        let id = self.allocate_id();
        // Replacement preserves uninterrupted startup registration evidence.
        // Removal/disconnect ends it; a later install receives a fresh time.
        let registered_at = self
            .probes
            .get(&owner)
            .map_or_else(Instant::now, |p| p.registered_at);
        let old = self.probes.insert(
            owner,
            ProbeEntry {
                id,
                remote,
                registered_at,
            },
        );
        Ok((id, old))
    }

    pub fn probe(&self, owner: u64) -> Option<&ProbeEntry<F>> {
        self.probes.get(&owner)
    }

    pub fn probe_registered_by(&self, owner: u64, deadline: Instant) -> bool {
        self.probe(owner)
            .is_some_and(|p| p.registered_at <= deadline)
    }

    pub fn remove_probe(&mut self, owner: u64, id: u64) -> Option<ProbeEntry<F>> {
        if self.probes.get(&owner)?.id != id {
            return None;
        }
        self.probes.remove(&owner)
    }

    pub fn install_watch(
        &mut self,
        owner: u64,
        policy: &AccessPolicy,
        buffer: Arc<WatchBuffer>,
    ) -> Result<WatchInstall, Status> {
        if !self.is_open(owner) {
            return Err(Status::Unavailable);
        }
        if !buffer.valid_capacity() {
            return Err(Status::InvalidArgument);
        }
        let id = self.allocate_id();
        let snapshot = self.list(owner, policy);
        let previous = self.watches.insert(
            owner,
            WatchEntry {
                id,
                buffer,
                policy: policy.clone(),
            },
        );
        Ok(WatchInstall {
            id,
            snapshot,
            previous,
        })
    }

    pub fn remove_watch(&mut self, owner: u64, id: u64) -> Option<WatchEntry> {
        if self.watches.get(&owner)?.id != id {
            return None;
        }
        self.watches.remove(&owner)
    }

    fn publish(&self, kind: ServiceChangeKind, info: &ServiceInfo) {
        let key = ServiceKey::new(&info.name, &info.interface_name);
        for watch in self.watches.values() {
            if watch.policy.can_connect(&key) {
                watch.buffer.push(kind.clone(), info);
            }
        }
    }

    pub fn owns_registration(&self, session: u64, key: &ServiceKey, version: u32) -> bool {
        self.is_open(session)
            && self
                .entries
                .get(key)
                .is_some_and(|entry| entry.owner == session && entry.info.version == version)
    }

    pub fn owns_registration_by(
        &self,
        session: u64,
        key: &ServiceKey,
        version: u32,
        deadline: Instant,
    ) -> bool {
        self.owns_registration(session, key, version)
            && self
                .entries
                .get(key)
                .is_some_and(|entry| entry.registered_at <= deadline)
    }

    pub fn register(
        &mut self,
        owner: u64,
        identity: &str,
        policy: &AccessPolicy,
        key: ServiceKey,
        version: u32,
        factory: F,
    ) -> Result<u64, (Status, F)> {
        let failure = if !self.is_open(owner) {
            Some(Status::Unavailable)
        } else if !key.is_valid() {
            Some(Status::InvalidArgument)
        } else if !policy.can_publish(&key) {
            Some(Status::PermissionDenied)
        } else if self.entries.contains_key(&key) {
            Some(Status::AlreadyRegistered)
        } else {
            None
        };
        if let Some(status) = failure {
            return Err((status, factory));
        }
        let generation = self.allocate_id();
        self.entries.insert(
            key.clone(),
            Entry {
                owner,
                info: ServiceInfo {
                    name: key.name.clone(),
                    interface_name: key.interface_name.clone(),
                    version,
                    generation,
                    provider: identity.to_owned(),
                },
                factory,
                registered_at: Instant::now(),
            },
        );
        self.publish(
            ServiceChangeKind::Added,
            &self.entries.get(&key).unwrap().info,
        );
        Ok(generation)
    }

    pub fn lookup(
        &self,
        session: u64,
        policy: &AccessPolicy,
        key: &ServiceKey,
        version: u32,
    ) -> Result<&Entry<F>, Status> {
        if !self.is_open(session) {
            return Err(Status::Unavailable);
        }
        if !key.is_valid() {
            return Err(Status::InvalidArgument);
        }
        if !policy.can_connect(key) {
            return Err(Status::PermissionDenied);
        }
        let entry = self.entries.get(key).ok_or(Status::NotFound)?;
        if entry.info.version != version {
            return Err(Status::IncompatibleVersion);
        }
        Ok(entry)
    }

    pub fn list(&self, session: u64, policy: &AccessPolicy) -> Vec<ServiceInfo> {
        if !self.is_open(session) {
            return Vec::new();
        }
        self.entries
            .iter()
            .filter(|(key, _)| policy.can_connect(key))
            .map(|(_, entry)| clone_info(&entry.info))
            .collect()
    }

    pub fn unregister(
        &mut self,
        owner: u64,
        key: &ServiceKey,
        generation: u64,
    ) -> Result<Entry<F>, Status> {
        if !self.is_open(owner) {
            return Err(Status::Unavailable);
        }
        let entry = self.entries.get(key).ok_or(Status::NotFound)?;
        if entry.owner != owner {
            return Err(Status::PermissionDenied);
        }
        if entry.info.generation != generation {
            return Err(Status::StaleGeneration);
        }
        let entry = self.entries.remove(key).unwrap();
        self.publish(ServiceChangeKind::Removed, &entry.info);
        Ok(entry)
    }

    pub fn remove_generation(&mut self, key: &ServiceKey, generation: u64) -> Option<Entry<F>> {
        if self.entries.get(key)?.info.generation != generation {
            return None;
        }
        let entry = self.entries.remove(key)?;
        self.publish(ServiceChangeKind::Removed, &entry.info);
        Some(entry)
    }

    pub fn close_session(
        &mut self,
        owner: u64,
    ) -> (Vec<Entry<F>>, Option<WatchEntry>, Option<ProbeEntry<F>>) {
        self.sessions.remove(&owner);
        let watch = self.watches.remove(&owner);
        let keys: Vec<_> = self
            .entries
            .iter()
            .filter(|(_, entry)| entry.owner == owner)
            .map(|(key, _)| key.clone())
            .collect();
        let removed: Vec<_> = keys
            .into_iter()
            .filter_map(|key| self.entries.remove(&key))
            .collect();
        for entry in &removed {
            self.publish(ServiceChangeKind::Removed, &entry.info);
        }
        (removed, watch, self.probes.remove(&owner))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn key() -> ServiceKey {
        ServiceKey::new("echo", "demo.Echo")
    }
    fn policy() -> AccessPolicy {
        AccessPolicy::new()
            .allow_publish(key())
            .allow_connect(key())
    }

    #[test]
    fn probes_are_session_owned_and_stale_disconnect_cannot_remove_replacement() {
        let mut registry = Registry::default();
        let owner = registry.open_session();
        let other = registry.open_session();
        let (first, old) = registry.install_probe(owner, 1).unwrap();
        let installed = registry.probe(owner).unwrap().registered_at;
        assert!(old.is_none());
        assert!(!registry.probe_registered_by(owner, installed - Duration::from_nanos(1)));
        assert!(registry.probe_registered_by(owner, installed));
        assert!(!registry.probe_registered_by(other, Instant::now()));
        let (second, old) = registry.install_probe(owner, 2).unwrap();
        assert_eq!(old.unwrap().remote, 1);
        assert!(registry.remove_probe(owner, first).is_none());
        assert_eq!(registry.probe(owner).unwrap().id, second);
        let (_, _, removed) = registry.close_session(owner);
        assert_eq!(removed.unwrap().remote, 2);
        assert!(registry.probe(owner).is_none());
        assert!(matches!(
            registry.install_probe(owner, 3),
            Err((Status::Unavailable, 3))
        ));
    }

    #[test]
    fn probe_replacement_preserves_continuous_readiness_but_reinstall_does_not() {
        let mut registry = Registry::default();
        let owner = registry.open_session();
        registry.install_probe(owner, ()).unwrap();
        let deadline = Instant::now() - Duration::from_millis(1);
        registry.probes.get_mut(&owner).unwrap().registered_at = deadline;
        let (second, _) = registry.install_probe(owner, ()).unwrap();
        assert!(registry.probe_registered_by(owner, deadline));
        registry.remove_probe(owner, second);
        registry.install_probe(owner, ()).unwrap();
        assert!(!registry.probe_registered_by(owner, deadline));
    }

    #[test]
    fn readiness_requires_exact_owner_and_version() {
        let mut registry = Registry::default();
        let first = registry.open_session();
        let second = registry.open_session();
        registry
            .register(first, "same-identity", &policy(), key(), 1, ())
            .unwrap();
        assert!(registry.owns_registration(first, &key(), 1));
        assert!(!registry.owns_registration(first, &key(), 2));
        assert!(!registry.owns_registration(second, &key(), 1));
        let registered_at = registry.entries.get(&key()).unwrap().registered_at;
        assert!(registry.owns_registration_by(first, &key(), 1, registered_at));
        assert!(!registry.owns_registration_by(
            first,
            &key(),
            1,
            registered_at - std::time::Duration::from_nanos(1)
        ));
        registry.close_session(first);
        assert!(!registry.owns_registration(first, &key(), 1));
    }

    #[test]
    fn duplicate_registration_cannot_replace_owner() {
        let mut registry = Registry::default();
        let first = registry.open_session();
        let second = registry.open_session();
        let generation = registry
            .register(first, "app-a", &policy(), key(), 1, ())
            .unwrap();
        assert_eq!(
            registry.register(second, "app-b", &policy(), key(), 1, ()),
            Err((Status::AlreadyRegistered, ()))
        );
        let entry = registry.lookup(second, &policy(), &key(), 1).unwrap();
        assert_eq!(entry.info.generation, generation);
        assert_eq!(entry.info.provider, "app-a");
        assert_eq!(
            registry.unregister(second, &key(), generation).err(),
            Some(Status::PermissionDenied)
        );
    }

    #[test]
    fn denied_peers_cannot_publish_discover_or_connect() {
        let mut registry = Registry::default();
        let owner = registry.open_session();
        let denied = AccessPolicy::new();
        assert_eq!(
            registry.register(owner, "app", &denied, key(), 1, ()),
            Err((Status::PermissionDenied, ()))
        );
        registry
            .register(owner, "app", &policy(), key(), 1, ())
            .unwrap();
        assert!(registry.list(owner, &denied).is_empty());
        assert_eq!(
            registry.lookup(owner, &denied, &key(), 1).err(),
            Some(Status::PermissionDenied)
        );
        assert_eq!(
            registry.lookup(owner, &policy(), &key(), 2).err(),
            Some(Status::IncompatibleVersion)
        );
    }

    #[test]
    fn stale_cleanup_cannot_remove_replacement() {
        let mut registry = Registry::default();
        let owner = registry.open_session();
        let old = registry
            .register(owner, "app", &policy(), key(), 1, ())
            .unwrap();
        registry.unregister(owner, &key(), old).unwrap();
        let new = registry
            .register(owner, "app", &policy(), key(), 1, ())
            .unwrap();
        assert!(new > old);
        assert!(registry.remove_generation(&key(), old).is_none());
        assert_eq!(
            registry.unregister(owner, &key(), old).err(),
            Some(Status::StaleGeneration)
        );
        assert_eq!(
            registry
                .lookup(owner, &policy(), &key(), 1)
                .unwrap()
                .info
                .generation,
            new
        );
    }

    #[test]
    fn closed_session_cannot_resurrect_or_remove_new_provider() {
        let mut registry = Registry::default();
        let old_owner = registry.open_session();
        let old = registry
            .register(old_owner, "app", &policy(), key(), 1, ())
            .unwrap();
        assert_eq!(registry.close_session(old_owner).0.len(), 1);
        assert_eq!(
            registry.register(old_owner, "app", &policy(), key(), 1, ()),
            Err((Status::Unavailable, ()))
        );
        let new_owner = registry.open_session();
        registry
            .register(new_owner, "app", &policy(), key(), 1, ())
            .unwrap();
        assert!(registry.close_session(old_owner).0.is_empty());
        assert!(registry.remove_generation(&key(), old).is_none());
        assert_eq!(registry.list(new_owner, &policy()).len(), 1);
        assert!(registry.list(old_owner, &policy()).is_empty());
    }

    #[test]
    fn watch_snapshot_and_filtered_revisions_cover_visible_replacement() {
        use crate::control::WatchStatus;
        let mut registry = Registry::default();
        let owner = registry.open_session();
        let viewer = registry.open_session();
        let hidden = ServiceKey::new("private", "demo.Echo");
        let publish = policy().allow_publish(hidden.clone());
        let old = registry
            .register(owner, "app", &publish, key(), 1, ())
            .unwrap();
        let secret = registry
            .register(owner, "app", &publish, hidden.clone(), 1, ())
            .unwrap();
        let buffer = WatchBuffer::for_test(4);
        let watch = registry
            .install_watch(viewer, &policy(), buffer.clone())
            .unwrap();
        assert_eq!(watch.snapshot.len(), 1);
        assert_eq!(watch.snapshot[0].generation, old);
        let (tx, rx) = std::sync::mpsc::channel();
        buffer.next(Box::new(move |s, r, c| {
            tx.send((s, r, c)).unwrap();
        }));
        registry.unregister(owner, &hidden, secret).unwrap();
        registry
            .register(owner, "app", &publish, hidden, 1, ())
            .unwrap();
        buffer.flush();
        assert!(rx.try_recv().is_err());
        registry.unregister(owner, &key(), old).unwrap();
        let new = registry
            .register(owner, "app", &publish, key(), 1, ())
            .unwrap();
        assert!(registry.remove_generation(&key(), old).is_none());
        buffer.flush();
        let (status, revision, changes) = rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!((status, revision), (WatchStatus::Updates, 2));
        assert_eq!(changes.len(), 2);
        assert_eq!(changes[0].service.generation, old);
        assert_eq!(changes[0].kind, ServiceChangeKind::Removed);
        assert_eq!(changes[1].service.generation, new);
        assert_eq!(changes[1].kind, ServiceChangeKind::Added);
    }

    #[test]
    fn watch_replacement_and_session_cleanup_cannot_remove_new_tokens() {
        let mut registry: Registry<()> = Registry::default();
        let viewer = registry.open_session();
        let first = WatchBuffer::for_test(4);
        let old = registry.install_watch(viewer, &policy(), first).unwrap();
        let second = WatchBuffer::for_test(4);
        let new = registry
            .install_watch(viewer, &policy(), second.clone())
            .unwrap();
        new.previous.unwrap().buffer.close();
        assert!(registry.remove_watch(viewer, old.id).is_none());
        let (tx, rx) = std::sync::mpsc::channel();
        second.next(Box::new(move |status, _, _| {
            tx.send(status).unwrap();
        }));
        let (_, removed, _) = registry.close_session(viewer);
        removed.unwrap().buffer.close();
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            crate::control::WatchStatus::Closed
        );
        assert!(registry.remove_watch(viewer, new.id).is_none());
        assert!(matches!(
            registry.install_watch(viewer, &policy(), WatchBuffer::for_test(4)),
            Err(Status::Unavailable)
        ));
        let fresh = registry.open_session();
        for capacity in [0, crate::MAX_WATCH_EVENTS + 1] {
            assert!(matches!(
                registry.install_watch(fresh, &policy(), WatchBuffer::for_test(capacity)),
                Err(Status::InvalidArgument)
            ));
        }
    }

    #[test]
    fn concurrent_snapshot_install_has_no_missing_or_duplicated_addition() {
        use std::sync::{Arc, Barrier, Mutex, mpsc};
        for _ in 0..16 {
            let mut registry = Registry::default();
            let owner = registry.open_session();
            let viewer = registry.open_session();
            let registry = Arc::new(Mutex::new(registry));
            let gate = Arc::new(Barrier::new(2));
            let buffer = WatchBuffer::for_test(4);
            let ((snapshot, _token), generation) = std::thread::scope(|scope| {
                let r = registry.clone();
                let g = gate.clone();
                let b = buffer.clone();
                let watch = scope.spawn(move || {
                    g.wait();
                    let install = r
                        .lock()
                        .unwrap()
                        .install_watch(viewer, &policy(), b)
                        .unwrap();
                    (install.snapshot, install.id)
                });
                let r = registry.clone();
                let g = gate.clone();
                let add = scope.spawn(move || {
                    g.wait();
                    r.lock()
                        .unwrap()
                        .register(owner, "app", &policy(), key(), 1, ())
                        .unwrap()
                });
                (watch.join().unwrap(), add.join().unwrap())
            });
            let (tx, rx) = mpsc::channel();
            buffer.next(Box::new(move |status, _, changes| {
                tx.send((status, changes)).unwrap();
            }));
            if snapshot.is_empty() {
                let (_, changes) = rx.recv_timeout(Duration::from_secs(1)).unwrap();
                assert_eq!(changes.len(), 1);
                assert_eq!(changes[0].service.generation, generation);
            } else {
                assert_eq!(snapshot.len(), 1);
                assert_eq!(snapshot[0].generation, generation);
                assert!(rx.try_recv().is_err());
                buffer.close();
            }
        }
    }
}
