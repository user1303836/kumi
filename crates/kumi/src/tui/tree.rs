//! The selected path open, other racks folded.
use super::icons::{device_kind, role_kind, IconKind};
use kumi_agent::core::contracts::{ChainNode, DeviceForm, DeviceNode, DeviceTree};
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TreeNode {
    Device,
    Chain,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TreeRole {
    Focus,
    Path,
    Other,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeRow {
    pub r#ref: String,
    pub node: TreeNode,
    pub kind: IconKind,
    pub name: String,
    pub prefix: String,
    pub count: Option<usize>,
    pub role: TreeRole,
    pub trail: Vec<String>,
    pub siblings: Vec<String>,
    /// What the DAW says of it beside its name: bypassed, offline, parallel.
    pub note: Option<String>,
}
#[derive(Clone, Debug, Default)]
pub struct TreeFocus {
    pub device: Option<String>,
    pub chain: Option<String>,
    pub device_ref: Option<String>,
    /// A container's insides can't be selected on their own (REAPER, Renoise): a focused one shows open, and its one
    /// unnamed chain is its devices.
    pub open_containers: bool,
}
fn appended(trail: &[String], refs: &[&str]) -> Vec<String> {
    trail.iter().cloned().chain(refs.iter().map(|s| (*s).to_string())).collect()
}
fn ref_path(devices: &[DeviceNode], reference: &str, trail: &[String]) -> Option<Vec<String>> {
    for item in devices {
        if item.r#ref == reference {
            return Some(appended(trail, &[&item.r#ref]));
        }
        for child in item.chains.as_deref().unwrap_or_default() {
            if let Some(found) =
                ref_path(child.devices.as_deref().unwrap_or_default(), reference, &appended(trail, &[&item.r#ref, &child.r#ref]))
            {
                return Some(found);
            }
        }
    }
    None
}
pub fn focus_path_refs(tree: &DeviceTree, device: Option<&str>, chain: Option<&str>, device_ref: Option<&str>) -> Option<Vec<String>> {
    if let Some(found) = device_ref.filter(|s| !s.is_empty()).and_then(|reference| ref_path(&tree.devices, reference, &[])) {
        return Some(found);
    }
    if let Some(device) = device.filter(|s| !s.is_empty()) {
        fn walk(
            devices: &[DeviceNode],
            device: &str,
            chain: Option<&str>,
            parent: Option<&str>,
            trail: &[String],
            found: &mut Vec<(Vec<String>, bool)>,
        ) {
            for item in devices {
                if item.name == device {
                    found.push((appended(trail, &[&item.r#ref]), parent == chain));
                }
                for child in item.chains.as_deref().unwrap_or_default() {
                    walk(
                        child.devices.as_deref().unwrap_or_default(),
                        device,
                        chain,
                        Some(&child.name),
                        &appended(trail, &[&item.r#ref, &child.r#ref]),
                        found,
                    );
                }
            }
        }
        let mut found = vec![];
        walk(&tree.devices, device, chain, None, &[], &mut found);
        if let Some((path, _)) = found.iter().find(|(_, preferred)| *preferred).or(found.first()) {
            return Some(path.iter().filter(|s| *s != "preferred").cloned().collect());
        }
    }
    let chain = chain.filter(|s| !s.is_empty())?;
    fn chain_path(devices: &[DeviceNode], chain: &str, trail: &[String]) -> Option<Vec<String>> {
        for item in devices {
            for child in item.chains.as_deref().unwrap_or_default() {
                let path = appended(trail, &[&item.r#ref, &child.r#ref]);
                if child.name == chain {
                    return Some(path);
                }
                if let Some(found) = chain_path(child.devices.as_deref().unwrap_or_default(), chain, &path) {
                    return Some(found);
                }
            }
        }
        None
    }
    chain_path(&tree.devices, chain, &[])
}
pub fn tree_rows(tree: &DeviceTree, focus: &TreeFocus) -> Vec<TreeRow> {
    let path = focus_path_refs(tree, focus.device.as_deref(), focus.chain.as_deref(), focus.device_ref.as_deref()).unwrap_or_default();
    struct Walker {
        path: Vec<String>,
        rows: Vec<TreeRow>,
        open_containers: bool,
    }
    impl Walker {
        fn role(&self, reference: &str) -> TreeRole {
            if self.path.last().is_some_and(|r| r == reference) {
                TreeRole::Focus
            } else if self.path.iter().any(|r| r == reference) {
                TreeRole::Path
            } else {
                TreeRole::Other
            }
        }
        fn devices(&mut self, devices: &[DeviceNode], lead: &str, trail: &[String]) {
            for (index, device) in devices.iter().enumerate() {
                let last = index == devices.len() - 1;
                let chains = device.chains.as_deref().unwrap_or_default();
                // A container of a DAW whose containers' insides can't be selected (REAPER's, Renoise's): its one unnamed
                // chain is its devices, and the focused one shows its lanes open.
                let own = self.open_containers && device.role.is_some_and(|role| role.form == DeviceForm::Container);
                let unnamed = own && chains.len() == 1 && chains[0].name.is_empty();
                let focused = self.path.last() == Some(&device.r#ref);
                let expanded = self.path.contains(&device.r#ref) && !chains.is_empty();
                let shown = if unnamed { chains[0].devices.as_deref().unwrap_or_default().len() } else { chains.len() };
                // What the DAW said it is, or what it says from the bridge's row.
                let kind = device.role.map(role_kind).unwrap_or_else(|| device_kind(&device.row()));
                self.rows.push(TreeRow {
                    r#ref: device.r#ref.clone(),
                    node: TreeNode::Device,
                    kind,
                    name: device.name.clone(),
                    prefix: format!("{lead}{}", if last { "└ " } else { "├ " }),
                    count: (!chains.is_empty() && !expanded).then_some(shown),
                    role: self.role(&device.r#ref),
                    trail: trail.to_vec(),
                    siblings: devices.iter().enumerate().filter(|(i, _)| *i != index).map(|(_, d)| d.name.clone()).collect(),
                    note: device.note.clone(),
                });
                if expanded {
                    let lead = format!("{lead}{}", if last { "  " } else { "│ " });
                    let trail = appended(trail, &[&device.name]);
                    if unnamed {
                        self.devices(chains[0].devices.as_deref().unwrap_or_default(), &lead, &trail);
                    } else {
                        self.chains(chains, &lead, &trail, device, own && focused);
                    }
                }
            }
        }
        fn chains(&mut self, chains: &[ChainNode], lead: &str, trail: &[String], rack: &DeviceNode, open: bool) {
            for (index, chain) in chains.iter().enumerate() {
                let last = index == chains.len() - 1;
                let devices = chain.devices.as_deref().unwrap_or_default();
                let expanded = (open || self.path.contains(&chain.r#ref)) && !devices.is_empty();
                self.rows.push(TreeRow {
                    r#ref: chain.r#ref.clone(),
                    node: TreeNode::Chain,
                    kind: if rack.can_have_drum_pads == Some(true) { IconKind::DrumPad } else { IconKind::Chain },
                    name: chain.name.clone(),
                    prefix: format!("{lead}{}", if last { "└ " } else { "├ " }),
                    count: (!devices.is_empty() && !expanded).then_some(devices.len()),
                    role: self.role(&chain.r#ref),
                    trail: trail.to_vec(),
                    siblings: chains.iter().enumerate().filter(|(i, _)| *i != index).map(|(_, c)| c.name.clone()).collect(),
                    note: None,
                });
                if expanded {
                    self.devices(devices, &format!("{lead}{}", if last { "  " } else { "│ " }), &appended(trail, &[&chain.name]));
                }
            }
        }
    }
    let mut walker = Walker { path, rows: vec![], open_containers: focus.open_containers };
    walker.devices(&tree.devices, "", &[]);
    walker.rows
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeWindow {
    pub rows: Vec<TreeRow>,
    pub above: usize,
    pub below: usize,
}
pub fn tree_window(rows: &[TreeRow], height: usize, keep: isize) -> TreeWindow {
    if rows.len() <= height {
        return TreeWindow { rows: rows.to_vec(), above: 0, below: 0 };
    }
    let start = (keep.max(0) as usize).saturating_sub(height / 2).min(rows.len() - height);
    TreeWindow { rows: rows[start..start + height].to_vec(), above: start, below: rows.len() - start - height }
}
