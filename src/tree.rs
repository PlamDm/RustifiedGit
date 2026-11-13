use crate::{fuzzy::Query, model::FileEntry};
use std::{
    collections::{BTreeMap, HashSet},
    ffi::OsStr,
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
struct Node<'a> {
    children: BTreeMap<&'a OsStr, Node<'a>>,
    file_index: Option<usize>,
    path: PathBuf,
    changed: bool,
    ignored: bool,
    directory: bool,
}

pub fn fuzzy_match(text: &str, needle: &str) -> bool {
    Query::new(needle).is_match(text)
}

pub fn rows(
    files: &[FileEntry],
    tree: bool,
    show_unchanged: bool,
    collapsed: &HashSet<PathBuf>,
    search: &str,
    changed_only: bool,
) -> Vec<Row> {
    let query = Query::new(search);
    let visible = |file: &FileEntry| file_visible(file, show_unchanged, changed_only, &query);
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
        let mut components = file.path.components().peekable();
        while let Some(component) = components.next() {
            path.push(component.as_os_str());
            let key = component.as_os_str();
            node = node.children.entry(key).or_insert_with(|| Node {
                path: path.clone(),
                ignored: true,
                ..Node::default()
            });
            node.changed |= file.changed();
            node.ignored &= file.ignored();
            let directory = components.peek().is_some();
            node.directory |= directory;
            if directory && search.is_empty() && collapsed.contains(&path) {
                break;
            }
            if !directory {
                node.file_index = Some(index);
            }
        }
    }
    fn flatten(
        node: &Node<'_>,
        files: &[FileEntry],
        depth: usize,
        collapsed: &HashSet<PathBuf>,
        search: &str,
        output: &mut Vec<Row>,
    ) {
        // BTreeMap already keeps siblings in native path order.
        for child in node
            .children
            .values()
            .filter(|child| child.directory)
            .chain(node.children.values().filter(|child| !child.directory))
        {
            let directory = child.directory;
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

pub(crate) fn file_visible(
    file: &FileEntry,
    show_unchanged: bool,
    changed_only: bool,
    query: &Query,
) -> bool {
    (show_unchanged || file.changed() || file.ignored())
        && (!changed_only || file.changed() || file.ignored())
        && query.is_match(&file.path.to_string_lossy())
}

pub fn matching_files<'a>(files: &'a [FileEntry], path: &Path) -> Vec<&'a FileEntry> {
    files
        .iter()
        .filter(|file| file.path == path || file.path.starts_with(path))
        .collect()
}
