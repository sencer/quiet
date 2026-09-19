use super::notification::Notification;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ClusterNode {
    Cluster(NotificationCluster),
    Leaf(Notification),
}

impl ClusterNode {
    pub fn len(&self) -> usize {
        match self {
            ClusterNode::Cluster(c) => c.len(),
            ClusterNode::Leaf(_) => 1,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn urgency(&self) -> u8 {
        match self {
            ClusterNode::Cluster(c) => c.urgency(),
            ClusterNode::Leaf(n) => n.urgency,
        }
    }

    pub fn best(&self) -> Option<&Notification> {
        match self {
            ClusterNode::Cluster(c) => c.best(),
            ClusterNode::Leaf(n) => Some(n),
        }
    }

    pub fn leafs(&self) -> Vec<Notification> {
        match self {
            ClusterNode::Cluster(c) => c.leafs(),
            ClusterNode::Leaf(n) => vec![n.clone()],
        }
    }
}

use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct NotificationCluster {
    pub children: BTreeMap<String, ClusterNode>,
    #[serde(skip)]
    cached_urgency: AtomicU8,
    #[serde(skip)]
    cached_len: AtomicUsize,
}

impl Clone for NotificationCluster {
    fn clone(&self) -> Self {
        Self {
            children: self.children.clone(),
            cached_urgency: AtomicU8::new(self.cached_urgency.load(Ordering::Relaxed)),
            cached_len: AtomicUsize::new(self.cached_len.load(Ordering::Relaxed)),
        }
    }
}

impl PartialEq for NotificationCluster {
    fn eq(&self, other: &Self) -> bool {
        self.children == other.children
    }
}

impl NotificationCluster {
    pub fn new() -> Self {
        Self {
            children: BTreeMap::new(),
            cached_urgency: AtomicU8::new(255),
            cached_len: AtomicUsize::new(usize::MAX),
        }
    }

    pub fn len(&self) -> usize {
        let cached = self.cached_len.load(Ordering::Relaxed);
        if cached != usize::MAX {
            return cached;
        }
        let total = self.children.values().map(|c| c.len()).sum();
        self.cached_len.store(total, Ordering::Relaxed);
        total
    }

    pub fn is_empty(&self) -> bool {
        self.children.is_empty()
    }

    pub fn urgency(&self) -> u8 {
        let cached = self.cached_urgency.load(Ordering::Relaxed);
        if cached != 255 {
            return cached;
        }
        let u = self.best().map(|n| n.urgency).unwrap_or(0);
        self.cached_urgency.store(u, Ordering::Relaxed);
        u
    }

    pub fn best(&self) -> Option<&Notification> {
        // Best is defined as highest urgency, with ties broken by latest created_at
        let mut best_notif: Option<&Notification> = None;

        for node in self.children.values() {
            if let Some(candidate) = node.best() {
                match best_notif {
                    None => best_notif = Some(candidate),
                    Some(current) => {
                        if candidate.urgency > current.urgency
                            || (candidate.urgency == current.urgency
                                && candidate.created_at > current.created_at)
                        {
                            best_notif = Some(candidate);
                        }
                    }
                }
            }
        }
        best_notif
    }

    pub fn leafs(&self) -> Vec<Notification> {
        let mut out = Vec::new();
        for node in self.children.values() {
            out.extend(node.leafs());
        }
        out
    }

    pub fn invalidate_cache(&self) {
        self.cached_urgency.store(255, Ordering::Relaxed);
        self.cached_len.store(usize::MAX, Ordering::Relaxed);
    }

    /// Recursively inserts a notification into the tree along the given path of keys.
    /// `keys` is e.g. `[app_name, body]` or `[app_name]`.
    /// The leaf level key is the stringified notification ID.
    pub fn insert(&mut self, keys: &[String], notification: Notification) {
        self.invalidate_cache();
        if keys.is_empty() {
            let id_key = notification.id.to_string();
            self.children
                .insert(id_key, ClusterNode::Leaf(notification));
            return;
        }

        let current_key = &keys[0];
        let remaining_keys = &keys[1..];

        let child_node = self
            .children
            .entry(current_key.clone())
            .or_insert_with(|| ClusterNode::Cluster(NotificationCluster::new()));

        match child_node {
            ClusterNode::Cluster(sub_cluster) => {
                sub_cluster.insert(remaining_keys, notification);
            }
            ClusterNode::Leaf(old_leaf) => {
                // If a leaf existed at this key, preserve old leaf in converted cluster
                let mut sub_cluster = NotificationCluster::new();
                let old_notif = old_leaf.clone();
                sub_cluster.insert(&[], old_notif);
                sub_cluster.insert(remaining_keys, notification);
                *child_node = ClusterNode::Cluster(sub_cluster);
            }
        }
    }

    /// Recursively removes a notification from the tree along the given path.
    /// Returns the removed notification if found.
    pub fn remove(&mut self, keys: &[String], id: u32) -> Option<Notification> {
        self.invalidate_cache();
        if keys.is_empty() {
            let id_key = id.to_string();
            if let Some(ClusterNode::Leaf(notif)) = self.children.remove(&id_key) {
                return Some(notif);
            }
            return None;
        }

        let current_key = &keys[0];
        let remaining_keys = &keys[1..];

        let mut removed = None;
        let mut should_delete_child = false;

        if let Some(child_node) = self.children.get_mut(current_key) {
            match child_node {
                ClusterNode::Cluster(sub_cluster) => {
                    removed = sub_cluster.remove(remaining_keys, id);
                    if sub_cluster.is_empty() {
                        should_delete_child = true;
                    }
                }
                ClusterNode::Leaf(notif) => {
                    if notif.id == id {
                        removed = Some(notif.clone());
                        should_delete_child = true;
                    }
                }
            }
        }

        if should_delete_child {
            self.children.remove(current_key);
        }

        removed
    }

    /// Removes an entire sub-branch along a path (e.g. `["Slack", "channel1"]`).
    pub fn remove_branch_path(&mut self, path: &[String]) -> Vec<Notification> {
        self.invalidate_cache();
        if path.is_empty() {
            return Vec::new();
        }

        if path.len() == 1 {
            return self.remove_branch(&path[0]);
        }

        let first = &path[0];
        let rest = &path[1..];

        let mut removed = Vec::new();
        let mut should_delete = false;

        if let Some(ClusterNode::Cluster(sub)) = self.children.get_mut(first) {
            removed = sub.remove_branch_path(rest);
            if sub.is_empty() {
                should_delete = true;
            }
        }

        if should_delete {
            self.children.remove(first);
        }

        removed
    }

    /// Removes an entire sub-branch at `key` (used for deleting a top-level group).
    pub fn remove_branch(&mut self, key: &str) -> Vec<Notification> {
        self.invalidate_cache();
        if let Some(node) = self.children.remove(key) {
            node.leafs()
        } else {
            Vec::new()
        }
    }

    /// Checks whether a context path exists in the tree.
    pub fn has_context(&self, context: &[String]) -> bool {
        let mut current = self;
        for key in context {
            if let Some(ClusterNode::Cluster(sub)) = current.children.get(key) {
                current = sub;
            } else {
                return false;
            }
        }
        true
    }

    /// Resolves a context path (e.g. `["Slack"]`) to a reference in the tree.
    /// Supports auto-descending when a cluster has only 1 child cluster.
    pub fn get_context<'a>(
        &'a self,
        context: &[String],
        auto_descend: bool,
    ) -> &'a NotificationCluster {
        self.resolve_context(context, auto_descend).1
    }

    /// Resolves a context path and returns both the fully resolved path (accounting for auto-descent)
    /// and the referenced cluster.
    pub fn resolve_context<'a>(
        &'a self,
        context: &[String],
        auto_descend: bool,
    ) -> (Vec<String>, &'a NotificationCluster) {
        let mut current = self;
        let mut resolved = Vec::new();

        for key in context {
            if let Some(ClusterNode::Cluster(sub)) = current.children.get(key) {
                resolved.push(key.clone());
                current = sub;
            } else {
                break;
            }
        }

        if auto_descend {
            while current.children.len() == 1 {
                if let Some((first_key, first_node)) = current.children.iter().next() {
                    match first_node {
                        ClusterNode::Cluster(sub) => {
                            // Don't auto-descend if at root and sub has multiple items
                            if std::ptr::eq(current, self) && context.is_empty() && sub.len() > 1 {
                                break;
                            }
                            resolved.push(first_key.clone());
                            current = sub;
                        }
                        ClusterNode::Leaf(_) => break,
                    }
                } else {
                    break;
                }
            }
        }

        (resolved, current)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cluster_insert_and_count() {
        let mut root = NotificationCluster::new();
        assert_eq!(root.len(), 0);
        assert!(root.is_empty());

        let n1 = Notification::new(
            1,
            "slack".into(),
            "".into(),
            "msg 1".into(),
            "body 1".into(),
            vec![],
            1,
            -1,
        );
        let n2 = Notification::new(
            2,
            "slack".into(),
            "".into(),
            "msg 2".into(),
            "body 2".into(),
            vec![],
            2, // Critical urgency
            -1,
        );
        let n3 = Notification::new(
            3,
            "mail".into(),
            "".into(),
            "email".into(),
            "body 3".into(),
            vec![],
            0, // Low urgency
            -1,
        );

        root.insert(&["slack".into(), "channel1".into()], n1.clone());
        root.insert(&["slack".into(), "channel1".into()], n2.clone());
        root.insert(&["mail".into()], n3.clone());

        assert_eq!(root.len(), 3);
        assert_eq!(root.urgency(), 2);
        assert_eq!(root.best().unwrap().id, 2);

        // Remove n2
        let removed = root.remove(&["slack".into(), "channel1".into()], 2);
        assert!(removed.is_some());
        assert_eq!(removed.unwrap().id, 2);
        assert_eq!(root.len(), 2);
        assert_eq!(root.urgency(), 1); // slack 1 is now highest
        assert_eq!(root.best().unwrap().id, 1);

        // Remove branch "slack"
        let removed_leafs = root.remove_branch("slack");
        assert_eq!(removed_leafs.len(), 1);
        assert_eq!(removed_leafs[0].id, 1);
        assert_eq!(root.len(), 1);
        assert_eq!(root.urgency(), 0);
        assert_eq!(root.best().unwrap().id, 3);
    }

    #[test]
    fn test_remove_branch_path_nested() {
        let mut root = NotificationCluster::new();
        let n1 = Notification::new(
            1,
            "chrome".into(),
            "".into(),
            "chat".into(),
            "hey".into(),
            vec![],
            1,
            -1,
        );
        let n2 = Notification::new(
            2,
            "chrome".into(),
            "".into(),
            "mail".into(),
            "inbox".into(),
            vec![],
            1,
            -1,
        );

        root.insert(&["chrome".into(), "chat.google.com".into()], n1);
        root.insert(&["chrome".into(), "mail.google.com".into()], n2);
        assert_eq!(root.len(), 2);

        // Remove nested branch ["chrome", "chat.google.com"]
        let removed = root.remove_branch_path(&["chrome".into(), "chat.google.com".into()]);
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].id, 1);
        assert_eq!(root.len(), 1);
        assert!(root.has_context(&["chrome".into(), "mail.google.com".into()]));
        assert!(!root.has_context(&["chrome".into(), "chat.google.com".into()]));
    }

    #[test]
    fn test_leaf_collision_preservation() {
        let mut root = NotificationCluster::new();
        let n1 = Notification::new(
            1,
            "slack".into(),
            "".into(),
            "leaf".into(),
            "b1".into(),
            vec![],
            1,
            -1,
        );
        let n2 = Notification::new(
            2,
            "slack".into(),
            "".into(),
            "sub".into(),
            "b2".into(),
            vec![],
            1,
            -1,
        );

        // First insert at path ["slack"]
        root.insert(&["slack".into()], n1);
        assert_eq!(root.len(), 1);

        // Now insert at deeper path ["slack", "general"]
        root.insert(&["slack".into(), "general".into()], n2);
        assert_eq!(root.len(), 2);

        // Verify both notifications can be queried
        let items = root.leafs();
        assert_eq!(items.len(), 2);
        let ids: Vec<u32> = items.iter().map(|n| n.id).collect();
        assert!(ids.contains(&1));
        assert!(ids.contains(&2));
    }

    #[test]
    fn test_resolve_context_auto_descent() {
        let mut root = NotificationCluster::new();
        let n1 = Notification::new(
            1,
            "app".into(),
            "".into(),
            "sum".into(),
            "body".into(),
            vec![],
            1,
            -1,
        );
        root.insert(&["a".into(), "b".into(), "c".into()], n1);

        let (resolved, cluster) = root.resolve_context(&["a".into()], true);
        assert_eq!(
            resolved,
            vec!["a".to_string(), "b".to_string(), "c".to_string()]
        );
        assert_eq!(cluster.len(), 1);
    }
}
