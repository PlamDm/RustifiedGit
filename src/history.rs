//! Cheap shared commit ordering and an index into the graph's visual rows.
//! Neither scrolling nor drawing copies or scans the loaded history.

use crate::model::Commit;
use std::{ops::Deref, sync::Arc};

#[derive(Clone, Debug, Default)]
pub struct CommitOrder(Arc<Vec<usize>>);

impl CommitOrder {
    pub fn new(indices: Vec<usize>) -> Self {
        Self(Arc::new(indices))
    }

    pub fn ptr_eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Deref for CommitOrder {
    type Target = [usize];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

pub struct CommitOrderIter {
    order: CommitOrder,
    next: usize,
}

impl Iterator for CommitOrderIter {
    type Item = usize;

    fn next(&mut self) -> Option<Self::Item> {
        let value = self.order.get(self.next).copied()?;
        self.next += 1;
        Some(value)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.order.len() - self.next;
        (remaining, Some(remaining))
    }
}

impl ExactSizeIterator for CommitOrderIter {}

impl IntoIterator for CommitOrder {
    type Item = usize;
    type IntoIter = CommitOrderIter;

    fn into_iter(self) -> Self::IntoIter {
        CommitOrderIter {
            order: self,
            next: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GraphRow {
    WorkingTree {
        commit_index: Option<usize>,
        logical: usize,
    },
    Commit {
        commit_index: usize,
        logical: usize,
    },
    Connector {
        commit_index: usize,
        line: usize,
    },
}

impl GraphRow {
    pub fn logical(self) -> Option<usize> {
        match self {
            Self::WorkingTree { logical, .. } | Self::Commit { logical, .. } => Some(logical),
            Self::Connector { .. } => None,
        }
    }
}

#[derive(Debug)]
pub struct GraphIndex {
    order: CommitOrder,
    searching: bool,
    // Search results have no connectors and therefore need no offsets array.
    starts: Vec<usize>,
    rows_without_worktree: usize,
}

impl GraphIndex {
    pub fn new(order: CommitOrder, commits: &[Commit], searching: bool) -> Self {
        let mut starts = Vec::new();
        let rows_without_worktree = if searching {
            order.len()
        } else {
            starts.reserve(order.len());
            let mut visual = 0;
            for index in order.iter().copied() {
                starts.push(visual);
                visual += commits[index].graph_before.len() + 1;
            }
            visual
        };
        Self {
            order,
            searching,
            starts,
            rows_without_worktree,
        }
    }

    pub fn matches(&self, order: &CommitOrder, searching: bool) -> bool {
        self.searching == searching && self.order.ptr_eq(order)
    }

    pub fn len(&self) -> usize {
        self.rows_without_worktree + 1
    }

    pub fn is_empty(&self) -> bool {
        false // The worktree row remains accessible even with no commits.
    }

    fn commit_base_visual(&self, filtered_index: usize) -> Option<usize> {
        if filtered_index >= self.order.len() {
            return None;
        }
        Some(if self.searching {
            filtered_index
        } else {
            self.starts
                .get(filtered_index + 1)
                .copied()
                .unwrap_or(self.rows_without_worktree)
                - 1
        })
    }

    pub fn working_tree_visual(&self, working_tree_index: usize) -> usize {
        self.commit_base_visual(working_tree_index).unwrap_or(0)
    }

    pub fn visual_for_logical(&self, logical: usize, working_tree_index: usize) -> Option<usize> {
        if logical == working_tree_index {
            return Some(self.working_tree_visual(working_tree_index));
        }
        let filtered = logical.saturating_sub(usize::from(logical > working_tree_index));
        self.commit_base_visual(filtered)
            .map(|base| base + usize::from(filtered >= working_tree_index))
    }

    pub fn row(&self, visual: usize, working_tree_index: usize) -> Option<GraphRow> {
        if visual >= self.len() {
            return None;
        }
        let worktree = self.working_tree_visual(working_tree_index);
        if visual == worktree {
            return Some(GraphRow::WorkingTree {
                commit_index: self.order.get(working_tree_index).copied(),
                logical: working_tree_index,
            });
        }
        let base = visual - usize::from(visual > worktree);
        let filtered = if self.searching {
            base
        } else {
            self.starts
                .partition_point(|start| *start <= base)
                .saturating_sub(1)
        };
        let commit_index = *self.order.get(filtered)?;
        let commit_visual = self.commit_base_visual(filtered)?;
        Some(if base == commit_visual {
            GraphRow::Commit {
                commit_index,
                logical: filtered + usize::from(filtered >= working_tree_index),
            }
        } else {
            GraphRow::Connector {
                commit_index,
                line: base - self.starts[filtered],
            }
        })
    }
}
