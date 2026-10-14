//! The derived `NodeId -> NodePath` index (§3.3) and tree navigation.

use std::collections::HashMap;

use crate::id::{NodeId, PageId};
use crate::model::{Document, Node, Page};

/// Where a node lives: page index, then child indices from the page root.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NodePath {
    pub page: usize,
    pub indices: Vec<usize>,
}

#[derive(Clone, Debug, Default)]
pub struct NodeIndex {
    map: HashMap<NodeId, NodePath>,
}

/// The parent of a node: a page root or a container node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParentRef {
    Page(PageId),
    Node(NodeId),
}

impl Document {
    /// A new empty document with one page.
    pub fn new(title: impl Into<String>, page: Page) -> Document {
        let mut doc = Document::default();
        doc.meta.title = title.into();
        doc.pages.push(page);
        doc.rebuild_index();
        doc
    }

    /// Rebuild the node index from the page trees. Commands call this after
    /// every change; call it yourself after editing pages directly.
    pub fn rebuild_index(&mut self) {
        let mut map = HashMap::new();
        fn visit(n: &Node, path: &mut NodePath, map: &mut HashMap<NodeId, NodePath>) {
            map.entry(n.id.clone()).or_insert_with(|| path.clone());
            if let Some(children) = n.children() {
                for (i, c) in children.iter().enumerate() {
                    path.indices.push(i);
                    visit(c, path, map);
                    path.indices.pop();
                }
            }
        }
        for (p, page) in self.pages.iter().enumerate() {
            let mut path = NodePath {
                page: p,
                indices: Vec::new(),
            };
            for (i, c) in page.children.iter().enumerate() {
                path.indices.push(i);
                visit(c, &mut path, &mut map);
                path.indices.pop();
            }
        }
        self.index = NodeIndex { map };
    }

    pub fn path_of(&self, id: &NodeId) -> Option<&NodePath> {
        self.index.map.get(id)
    }

    /// All indexed node IDs (page trees only, not component masters).
    pub fn node_ids(&self) -> impl Iterator<Item = &NodeId> {
        self.index.map.keys()
    }

    pub fn node(&self, id: &NodeId) -> Option<&Node> {
        let path = self.path_of(id)?;
        let (first, rest) = path.indices.split_first()?;
        let mut n = self.pages.get(path.page)?.children.get(*first)?;
        for i in rest {
            n = n.children()?.get(*i)?;
        }
        (&n.id == id).then_some(n)
    }

    pub fn node_mut(&mut self, id: &NodeId) -> Option<&mut Node> {
        let path = self.index.map.get(id)?.clone();
        let (first, rest) = path.indices.split_first()?;
        let mut n = self.pages.get_mut(path.page)?.children.get_mut(*first)?;
        for i in rest {
            n = n.children_mut()?.get_mut(*i)?;
        }
        (&n.id == id).then_some(n)
    }

    /// A node of a page, or of a component master (which the index does not
    /// cover).
    pub fn node_or_master(&self, id: &NodeId) -> Option<&Node> {
        self.node(id).or_else(|| {
            let mut found = None;
            for c in &self.components {
                c.root.walk(&mut |n| {
                    if &n.id == id {
                        found = Some(n);
                    }
                });
            }
            found
        })
    }

    pub fn node_or_master_mut(&mut self, id: &NodeId) -> Option<&mut Node> {
        if self.path_of(id).is_some() {
            return self.node_mut(id);
        }
        fn find<'a>(n: &'a mut Node, id: &NodeId) -> Option<&'a mut Node> {
            if &n.id == id {
                return Some(n);
            }
            n.children_mut()?.iter_mut().find_map(|c| find(c, id))
        }
        self.components
            .iter_mut()
            .find_map(|c| find(&mut c.root, id))
    }

    pub fn page(&self, id: &PageId) -> Option<&Page> {
        self.pages.iter().find(|p| &p.id == id)
    }

    pub fn page_index(&self, id: &PageId) -> Option<usize> {
        self.pages.iter().position(|p| &p.id == id)
    }

    /// The parent of a node and the node's index in it.
    pub fn parent_of(&self, id: &NodeId) -> Option<(ParentRef, usize)> {
        let path = self.path_of(id)?;
        let (last, ancestors) = path.indices.split_last()?;
        if ancestors.is_empty() {
            return Some((ParentRef::Page(self.pages[path.page].id.clone()), *last));
        }
        let mut n = &self.pages[path.page].children[ancestors[0]];
        for i in &ancestors[1..] {
            n = &n.children()?[*i];
        }
        Some((ParentRef::Node(n.id.clone()), *last))
    }

    /// The children list of a parent, if the parent exists and stores children.
    pub fn children_of(&self, parent: &ParentRef) -> Option<&Vec<Node>> {
        match parent {
            ParentRef::Page(id) => self.page(id).map(|p| &p.children),
            ParentRef::Node(id) => self.node(id)?.children(),
        }
    }

    pub fn children_of_mut(&mut self, parent: &ParentRef) -> Option<&mut Vec<Node>> {
        match parent {
            ParentRef::Page(id) => self
                .pages
                .iter_mut()
                .find(|p| &p.id == id)
                .map(|p| &mut p.children),
            ParentRef::Node(id) => self.node_mut(id)?.children_mut(),
        }
    }

    /// Calls `f` on every node of every page, depth first.
    pub fn walk_nodes<'a>(&'a self, f: &mut impl FnMut(&'a Node)) {
        for page in &self.pages {
            for n in &page.children {
                n.walk(f);
            }
        }
    }
}
