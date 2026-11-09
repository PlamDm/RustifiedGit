use crate::model::FileEntry;
use std::{
    collections::{BTreeMap, HashSet},
    path::{Path, PathBuf},
};

#[derive(Clone, Debug)]
pub struct Row {
    pub path: PathBuf,
    pub label: String,
    pub depth: usize,
    pub directory: bool,
    pub changed: bool,
    pub ignored: bool,
    pub status: String,
    pub file_index: Option<usize>,
}

#[derive(Default)]
struct Node {
    children: BTreeMap<String, Node>,
    file_index: Option<usize>,
    path: PathBuf,
    changed: bool,
    ignored: bool,
}

pub fn fuzzy_match(text: &str, needle: &str) -> bool {
    let text = text.to_lowercase();
    let needle = needle.to_lowercase();
    let mut chars = text.chars();
    needle
        .chars()
        .all(|wanted| chars.by_ref().any(|actual| actual == wanted))
}

pub fn rows(
    files: &[FileEntry],
    tree: bool,
    show_unchanged: bool,
    collapsed: &HashSet<PathBuf>,
    search: &str,
    changed_only: bool,
) -> Vec<Row> {
    let visible = |file: &FileEntry| {
        (show_unchanged || file.changed() || file.ignored())
            && (!changed_only || file.changed())
            && fuzzy_match(&file.path.to_string_lossy(), search)
    };
    if !tree {
        return files
            .iter()
            .enumerate()
            .filter(|(_, file)| visible(file))
            .map(|(index, file)| Row {
                path: file.path.clone(),
                label: file.path.to_string_lossy().into_owned(),
                depth: 0,
                directory: false,
                changed: file.changed(),
                ignored: file.ignored(),
                status: file.status(),
                file_index: Some(index),
            })
            .collect();
    }
    let mut root = Node::default();
    for (index, file) in files.iter().enumerate().filter(|(_, file)| visible(file)) {
        let mut node = &mut root;
        let mut path = PathBuf::new();
        for component in file.path.components() {
            path.push(component.as_os_str());
            // Paths stay native in the value; the key only controls display ordering.
            let key = format!(
                "{}\0{:?}",
                component.as_os_str().to_string_lossy(),
                component.as_os_str()
            );
            node = node.children.entry(key).or_insert_with(|| Node {
                path: path.clone(),
                ignored: true,
                ..Node::default()
            });
            node.changed |= file.changed();
            node.ignored &= file.ignored();
        }
        node.file_index = Some(index);
    }
    fn flatten(
        node: &Node,
        files: &[FileEntry],
        depth: usize,
        collapsed: &HashSet<PathBuf>,
        search: &str,
        output: &mut Vec<Row>,
    ) {
        let mut children: Vec<_> = node.children.values().collect();
        children.sort_by_key(|node| (node.children.is_empty(), &node.path));
        for child in children {
            let directory = !child.children.is_empty();
            output.push(Row {
                path: child.path.clone(),
                label: child
                    .path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                depth,
                directory,
                changed: child.changed,
                ignored: child.ignored,
                status: child
                    .file_index
                    .map(|index| files[index].status())
                    .unwrap_or_else(|| {
                        if child.changed {
                            " •".into()
                        } else {
                            "  ".into()
                        }
                    }),
                file_index: child.file_index,
            });
            if directory && (!collapsed.contains(&child.path) || !search.is_empty()) {
                flatten(child, files, depth + 1, collapsed, search, output);
            }
        }
    }
    let mut output = Vec::with_capacity(files.len());
    flatten(&root, files, 0, collapsed, search, &mut output);
    output
}

pub fn matching_files<'a>(files: &'a [FileEntry], path: &Path) -> Vec<&'a FileEntry> {
    files
        .iter()
        .filter(|file| file.path == path || file.path.starts_with(path))
        .collect()
}
