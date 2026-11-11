use crate::model::Reference;
use std::collections::{BTreeMap, HashSet};

#[derive(Clone, Debug)]
pub struct BranchRow {
    pub path: String,
    pub label: String,
    pub depth: usize,
    pub reference: Option<usize>,
    pub descendants: Vec<usize>,
}

impl BranchRow {
    pub fn is_group(&self) -> bool {
        self.reference.is_none()
    }
}

#[derive(Default)]
struct Node {
    children: BTreeMap<String, Node>,
    reference: Option<usize>,
    descendants: Vec<usize>,
}

pub fn rows(
    references: &[Reference],
    indices: &[usize],
    delimiter: &str,
    collapsed: &HashSet<String>,
    searching: bool,
) -> Vec<BranchRow> {
    if delimiter.is_empty() {
        return indices
            .iter()
            .map(|index| BranchRow {
                path: references[*index].name.clone(),
                label: references[*index].name.clone(),
                depth: 0,
                reference: Some(*index),
                descendants: vec![*index],
            })
            .collect();
    }
    let mut root = Node::default();
    for index in indices {
        let mut node = &mut root;
        for part in references[*index].name.split(delimiter) {
            node = node.children.entry(part.to_owned()).or_default();
            node.descendants.push(*index);
        }
        node.reference = Some(*index);
    }
    fn flatten(
        node: &Node,
        prefix: &str,
        depth: usize,
        delimiter: &str,
        collapsed: &HashSet<String>,
        searching: bool,
        rows: &mut Vec<BranchRow>,
    ) {
        for (name, child) in node
            .children
            .iter()
            .filter(|(_, child)| !child.children.is_empty())
            .chain(
                node.children
                    .iter()
                    .filter(|(_, child)| child.children.is_empty()),
            )
        {
            let path = if depth == 0 {
                name.clone()
            } else {
                format!("{prefix}{delimiter}{name}")
            };
            rows.push(BranchRow {
                path: path.clone(),
                label: if name.is_empty() {
                    delimiter.to_owned()
                } else {
                    name.clone()
                },
                depth,
                reference: if child.children.is_empty() {
                    child.reference
                } else {
                    None
                },
                descendants: child.descendants.clone(),
            });
            if searching || !collapsed.contains(&path) {
                if !child.children.is_empty() {
                    if let Some(reference) = child.reference {
                        rows.push(BranchRow {
                            path: path.clone(),
                            label: name.clone(),
                            depth: depth + 1,
                            reference: Some(reference),
                            descendants: vec![reference],
                        });
                    }
                }
                flatten(
                    child,
                    &path,
                    depth + 1,
                    delimiter,
                    collapsed,
                    searching,
                    rows,
                );
            }
        }
    }
    let mut result = Vec::new();
    flatten(&root, "", 0, delimiter, collapsed, searching, &mut result);
    result
}
