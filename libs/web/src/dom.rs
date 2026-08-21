//! The document tree.
//!
//! html5ever does the parsing (via `markup5ever_rcdom`), and then we immediately convert its
//! `Rc`/`RefCell` tree into a flat arena: `Vec<Node>` addressed by `NodeId`.
//!
//! That conversion is deliberate, not ceremony. Everything downstream — style, layout, paint, and
//! eventually the DOM bindings for JS — needs to hang side tables off nodes and walk the tree
//! repeatedly. With an arena those are `Vec`s indexed by `NodeId`, ancestor walks are a `parent`
//! field, and there is no reference-counting traffic or `RefCell` borrow risk on a hot path. `rcdom`
//! is a fine parse target and a poor engine representation.

use html5ever::tendril::TendrilSink;
use html5ever::{parse_document, ParseOpts};
use markup5ever_rcdom::{Handle, NodeData, RcDom};

pub type NodeId = usize;

#[derive(Debug, Clone, PartialEq)]
pub enum NodeKind {
    Document,
    Element { name: String, attrs: Vec<(String, String)> },
    Text(String),
    Comment(String),
    Doctype,
}

#[derive(Debug, Clone)]
pub struct Node {
    pub kind: NodeKind,
    pub parent: Option<NodeId>,
    pub children: Vec<NodeId>,
}

impl Node {
    /// Lowercased tag name, or `None` for anything that is not an element.
    pub fn tag(&self) -> Option<&str> {
        match &self.kind {
            NodeKind::Element { name, .. } => Some(name),
            _ => None,
        }
    }

    pub fn attr(&self, name: &str) -> Option<&str> {
        match &self.kind {
            // Attribute names are already lowercased at build time; compare case-insensitively
            // anyway so callers can't be tripped up by passing "Href".
            NodeKind::Element { attrs, .. } => attrs
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(name))
                .map(|(_, v)| v.as_str()),
            _ => None,
        }
    }

    pub fn text(&self) -> Option<&str> {
        match &self.kind {
            NodeKind::Text(t) => Some(t),
            _ => None,
        }
    }

    /// `class` split on whitespace. HTML allows any run of whitespace as the separator.
    pub fn classes(&self) -> impl Iterator<Item = &str> {
        self.attr("class").unwrap_or("").split_ascii_whitespace()
    }

    pub fn id(&self) -> Option<&str> {
        self.attr("id")
    }
}

#[derive(Debug, Clone)]
pub struct Dom {
    pub nodes: Vec<Node>,
    pub root: NodeId,
}

impl Dom {
    pub fn parse(html: &str) -> Dom {
        let rc: RcDom = parse_document(RcDom::default(), ParseOpts::default())
            .from_utf8()
            .read_from(&mut html.as_bytes())
            .expect("parsing from a &str is infallible");

        let mut dom = Dom { nodes: Vec::new(), root: 0 };
        dom.root = dom.push(NodeKind::Document, None);
        let root = dom.root;
        dom.convert_children(&rc.document, root);
        dom
    }

    fn push(&mut self, kind: NodeKind, parent: Option<NodeId>) -> NodeId {
        let id = self.nodes.len();
        self.nodes.push(Node { kind, parent, children: Vec::new() });
        if let Some(p) = parent {
            self.nodes[p].children.push(id);
        }
        id
    }

    fn convert_children(&mut self, handle: &Handle, parent: NodeId) {
        for child in handle.children.borrow().iter() {
            let kind = match &child.data {
                NodeData::Document => Some(NodeKind::Document),
                NodeData::Doctype { .. } => Some(NodeKind::Doctype),
                NodeData::Text { contents } => {
                    Some(NodeKind::Text(contents.borrow().to_string()))
                }
                NodeData::Comment { contents } => {
                    Some(NodeKind::Comment(contents.to_string()))
                }
                NodeData::Element { name, attrs, .. } => {
                    // Lowercase once, here. HTML tag and attribute names are ASCII
                    // case-insensitive, and normalising at the boundary means selector matching
                    // and every later lookup can use plain comparisons.
                    let attrs = attrs
                        .borrow()
                        .iter()
                        .map(|a| {
                            (a.name.local.to_ascii_lowercase().to_string(), a.value.to_string())
                        })
                        .collect();
                    Some(NodeKind::Element {
                        name: name.local.to_ascii_lowercase().to_string(),
                        attrs,
                    })
                }
                // Processing instructions and template contents carry nothing we render.
                _ => None,
            };
            if let Some(kind) = kind {
                let id = self.push(kind, Some(parent));
                self.convert_children(child, id);
            }
        }
    }

    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id]
    }

    /// Depth-first document order — the order style and layout both want.
    pub fn descendants(&self, start: NodeId) -> Vec<NodeId> {
        let mut out = Vec::new();
        let mut stack = vec![start];
        while let Some(id) = stack.pop() {
            out.push(id);
            // Push in reverse so children come off the stack left-to-right.
            for &c in self.nodes[id].children.iter().rev() {
                stack.push(c);
            }
        }
        out
    }

    /// First element with this tag, in document order.
    pub fn find_tag(&self, tag: &str) -> Option<NodeId> {
        self.descendants(self.root)
            .into_iter()
            .find(|&id| self.nodes[id].tag() == Some(tag))
    }

    /// Concatenated text of a subtree, as `textContent` would give it.
    pub fn text_content(&self, id: NodeId) -> String {
        let mut out = String::new();
        for d in self.descendants(id) {
            if let Some(t) = self.nodes[d].text() {
                out.push_str(t);
            }
        }
        out
    }

    /// The contents of `<title>`, trimmed — what goes in a window title bar.
    pub fn title(&self) -> Option<String> {
        let id = self.find_tag("title")?;
        let t = self.text_content(id).trim().to_string();
        if t.is_empty() { None } else { Some(t) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_the_implied_document_structure() {
        // No <html>, <head> or <body> in the source: the parser must synthesise them.
        let dom = Dom::parse("<p>hi</p>");
        assert!(dom.find_tag("html").is_some());
        assert!(dom.find_tag("head").is_some());
        assert!(dom.find_tag("body").is_some());
        let p = dom.find_tag("p").unwrap();
        assert_eq!(dom.text_content(p), "hi");
    }

    #[test]
    fn repairs_unclosed_and_misnested_tags() {
        // This is exactly why html5ever is worth the dependency.
        let dom = Dom::parse("<body><p>one<p>two</body>");
        let ps: Vec<_> = dom
            .descendants(dom.root)
            .into_iter()
            .filter(|&id| dom.node(id).tag() == Some("p"))
            .collect();
        assert_eq!(ps.len(), 2, "an unclosed <p> must be closed implicitly");
        assert_eq!(dom.text_content(ps[0]).trim(), "one");
        assert_eq!(dom.text_content(ps[1]).trim(), "two");
    }

    #[test]
    fn lowercases_tag_and_attribute_names() {
        let dom = Dom::parse(r#"<A HREF="/x">link</A>"#);
        let a = dom.find_tag("a").expect("tag name normalised to lowercase");
        assert_eq!(dom.node(a).attr("href"), Some("/x"));
        // Attribute VALUES keep their case; only names are normalised.
        assert_eq!(dom.node(a).attr("HREF"), Some("/x"));
    }

    #[test]
    fn parent_links_are_consistent() {
        let dom = Dom::parse("<div><span>x</span></div>");
        let span = dom.find_tag("span").unwrap();
        let div = dom.find_tag("div").unwrap();
        assert_eq!(dom.node(span).parent, Some(div));
        assert!(dom.node(div).children.contains(&span));
    }

    #[test]
    fn extracts_title_and_classes() {
        let dom = Dom::parse(
            "<html><head><title> Hello </title></head><body><p class='a  b'>x</p></body></html>",
        );
        assert_eq!(dom.title().as_deref(), Some("Hello"));
        let p = dom.find_tag("p").unwrap();
        let classes: Vec<_> = dom.node(p).classes().collect();
        assert_eq!(classes, vec!["a", "b"]);
    }

    #[test]
    fn descendants_are_in_document_order() {
        let dom = Dom::parse("<body><i>1</i><b>2</b></body>");
        let tags: Vec<_> = dom
            .descendants(dom.root)
            .into_iter()
            .filter_map(|id| dom.node(id).tag().map(str::to_string))
            .collect();
        let i = tags.iter().position(|t| t == "i").unwrap();
        let b = tags.iter().position(|t| t == "b").unwrap();
        assert!(i < b, "document order, not reversed");
    }

    #[test]
    fn comments_are_kept_but_carry_no_text() {
        let dom = Dom::parse("<body><!-- note -->visible</body>");
        let body = dom.find_tag("body").unwrap();
        assert_eq!(dom.text_content(body), "visible");
    }
}
