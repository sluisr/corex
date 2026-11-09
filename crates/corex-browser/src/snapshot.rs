use std::collections::{HashMap, HashSet};
use std::fmt::Write;

use serde_json::Value;

const MAX_OUTPUT: usize = 30_000;
const MAX_NAME: usize = 120;

const INTERACTIVE: &[&str] = &[
    "link",
    "button",
    "textbox",
    "searchbox",
    "checkbox",
    "radio",
    "combobox",
    "menuitem",
    "menuitemcheckbox",
    "menuitemradio",
    "tab",
    "switch",
    "slider",
    "spinbutton",
    "option",
    "treeitem",
    "listbox",
    "DisclosureTriangle",
    "PopUpButton",
    "ColorWell",
    "DateTime",
    "Date",
    "Time",
];

const TRANSPARENT: &[&str] = &["none", "generic", "presentation", "InlineTextBox", "LineBreak", "RootWebArea", "LabelText", "MenuListPopup"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeRef {
    pub backend: i64,
    pub session: Option<String>,
    pub owner: Option<i64>,
}

pub struct Snapshot {
    pub text: String,
    pub refs: HashMap<String, NodeRef>,
}

fn text_of(node: &Value, key: &str) -> String {
    node[key]["value"].as_str().unwrap_or("").trim().to_string()
}

fn property(node: &Value, name: &str) -> Option<String> {
    node["properties"].as_array()?.iter().find_map(|p| {
        if p["name"] == name {
            match &p["value"]["value"] {
                Value::String(s) => Some(s.clone()),
                other => Some(other.to_string()),
            }
        } else {
            None
        }
    })
}

fn shorten(text: &str, max: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        flat
    } else {
        let cut: String = flat.chars().take(max).collect();
        format!("{cut}…")
    }
}

struct Walker<'a> {
    nodes: HashMap<&'a str, &'a Value>,
    seen: HashSet<&'a str>,
    out: String,
    refs: HashMap<String, NodeRef>,
    counter: usize,
    limit: usize,
    query: Option<String>,
}

impl Walker<'_> {
    fn emit(&mut self, line: &str) {
        if self.query.as_ref().is_none_or(|query| line.to_lowercase().contains(query)) {
            self.out.push_str(line);
            self.out.push('\n');
        }
    }
}

impl<'a> Walker<'a> {
    fn visit(&mut self, id: &'a str, depth: usize, parent_named: bool) {
        if self.out.len() > self.limit || !self.seen.insert(id) {
            return;
        }
        let Some(&node) = self.nodes.get(id) else { return };
        let children: Vec<&str> = node["childIds"]
            .as_array()
            .map(|ids| ids.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        let role = text_of(node, "role");
        let name = text_of(node, "name");
        let ignored = node["ignored"].as_bool().unwrap_or(false);
        let aria_hidden = ignored
            && node["ignoredReasons"].as_array().is_some_and(|reasons| {
                reasons.iter().any(|r| r["name"].as_str().is_some_and(|n| n.starts_with("ariaHidden")))
            });
        if aria_hidden {
            return;
        }
        let hidden = ignored || TRANSPARENT.contains(&role.as_str());

        if hidden {
            for child in children {
                self.visit(child, depth, parent_named);
            }
            return;
        }

        if role == "StaticText" {
            if !name.is_empty() && !parent_named {
                self.emit(&format!("{}- text \"{}\"", "  ".repeat(depth), shorten(&name, MAX_NAME)));
            }
            return;
        }

        let mut line = format!("{}- {}", "  ".repeat(depth), role);
        if !name.is_empty() {
            let _ = write!(line, " \"{}\"", shorten(&name, MAX_NAME));
        }
        if INTERACTIVE.contains(&role.as_str()) {
            if let Some(backend) = node["backendDOMNodeId"].as_i64() {
                self.counter += 1;
                let key = format!("e{}", self.counter);
                let _ = write!(line, " [ref={key}]");
                let node_ref = NodeRef {
                    backend,
                    session: node["corexSession"].as_str().map(str::to_string),
                    owner: node["corexOwner"].as_i64(),
                };
                self.refs.insert(key, node_ref);
            }
        }
        let value = text_of(node, "value");
        if !value.is_empty() {
            let _ = write!(line, " value=\"{}\"", shorten(&value, 80));
        }
        if let Some(level) = property(node, "level") {
            let _ = write!(line, " level={level}");
        }
        for flag in ["checked", "pressed", "expanded", "selected", "disabled", "required", "readonly", "invalid", "focused", "modal"] {
            if let Some(state) = property(node, flag) {
                if state != "false" {
                    let _ = write!(line, " [{flag}={state}]");
                }
            }
        }
        let description = text_of(node, "description");
        if !description.is_empty() && description != name {
            let _ = write!(line, " description=\"{}\"", shorten(&description, 100));
        }
        if role == "link" && (name.is_empty() || self.query.is_some()) {
            if let Some(url) = property(node, "url") {
                let _ = write!(line, " -> {}", shorten(&url, 100));
            }
        }
        self.emit(&line);

        let named = !name.is_empty();
        for child in children {
            self.visit(child, depth + 1, named);
        }
    }
}

pub fn build(tree: &Value, query: Option<&str>) -> Snapshot {
    let list: &[Value] = tree["nodes"].as_array().map(Vec::as_slice).unwrap_or(&[]);
    let nodes: HashMap<&str, &Value> = list
        .iter()
        .filter_map(|n| n["nodeId"].as_str().map(|id| (id, n)))
        .collect();
    let mut walker = Walker {
        nodes,
        seen: HashSet::new(),
        out: String::new(),
        refs: HashMap::new(),
        counter: 0,
        limit: if query.is_some() { usize::MAX } else { MAX_OUTPUT },
        query: query.map(str::to_lowercase),
    };
    if let Some(root) = list.first().and_then(|n| n["nodeId"].as_str()) {
        walker.visit(root, 0, false);
    }
    let mut text = walker.out;
    if text.len() > MAX_OUTPUT {
        let mut end = MAX_OUTPUT;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text.push_str(
            "\n… snapshot truncated; refs below this point were not assigned. Use snapshot with a query \
             (for example the label of the field you need) to list matching elements from the whole page\n",
        );
    }
    Snapshot { text, refs: walker.refs }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn assigns_refs_to_interactive_nodes_and_skips_noise() {
        let tree = json!({"nodes": [
            {"nodeId": "1", "role": {"value": "RootWebArea"}, "name": {"value": "Home"}, "childIds": ["2", "3", "5"]},
            {"nodeId": "2", "role": {"value": "heading"}, "name": {"value": "Welcome"},
             "properties": [{"name": "level", "value": {"value": 1}}], "childIds": ["4"]},
            {"nodeId": "4", "role": {"value": "StaticText"}, "name": {"value": "Welcome"}, "childIds": []},
            {"nodeId": "3", "role": {"value": "button"}, "name": {"value": "Go"}, "backendDOMNodeId": 42, "childIds": []},
            {"nodeId": "5", "role": {"value": "generic"}, "ignored": true, "childIds": []}
        ]});
        let snap = build(&tree, None);
        assert_eq!(snap.refs.get("e1"), Some(&NodeRef { backend: 42, session: None, owner: None }));
        assert!(snap.text.contains("- button \"Go\" [ref=e1]"));
        assert!(snap.text.contains("- heading \"Welcome\" level=1"));
        assert!(!snap.text.contains("text \"Welcome\""));
    }

    #[test]
    fn remembers_the_session_and_owner_of_nodes_from_other_frames() {
        let tree = json!({"nodes": [
            {"nodeId": "1", "role": {"value": "RootWebArea"}, "childIds": ["2"]},
            {"nodeId": "2", "role": {"value": "button"}, "name": {"value": "Pay"}, "backendDOMNodeId": 5,
             "corexSession": "7:abc", "corexOwner": 3, "childIds": []}
        ]});
        let snap = build(&tree, None);
        assert_eq!(
            snap.refs.get("e1"),
            Some(&NodeRef { backend: 5, session: Some("7:abc".into()), owner: Some(3) })
        );
    }

    #[test]
    fn gives_refs_to_disclosure_triangles_and_skips_aria_hidden_subtrees() {
        let tree = json!({"nodes": [
            {"nodeId": "1", "role": {"value": "RootWebArea"}, "childIds": ["2", "3"]},
            {"nodeId": "2", "role": {"value": "DisclosureTriangle"}, "name": {"value": "Advanced"},
             "backendDOMNodeId": 7, "childIds": []},
            {"nodeId": "3", "role": {"value": "generic"}, "ignored": true,
             "ignoredReasons": [{"name": "ariaHiddenElement"}], "childIds": ["4"]},
            {"nodeId": "4", "role": {"value": "button"}, "name": {"value": "Overlay"}, "backendDOMNodeId": 9, "childIds": []}
        ]});
        let snap = build(&tree, None);
        assert!(snap.text.contains("- DisclosureTriangle \"Advanced\" [ref=e1]"));
        assert!(!snap.text.contains("Overlay"));
    }

    #[test]
    fn filters_by_query_and_shows_validation_state() {
        let tree = json!({"nodes": [
            {"nodeId": "1", "role": {"value": "RootWebArea"}, "childIds": ["2", "3"]},
            {"nodeId": "2", "role": {"value": "button"}, "name": {"value": "Save"}, "backendDOMNodeId": 4, "childIds": []},
            {"nodeId": "3", "role": {"value": "textbox"}, "name": {"value": "Email"}, "backendDOMNodeId": 5,
             "description": {"value": "Enter a valid address"},
             "properties": [{"name": "invalid", "value": {"value": "true"}}], "childIds": []}
        ]});
        let snap = build(&tree, Some("email"));
        assert_eq!(
            snap.text.trim(),
            "- textbox \"Email\" [ref=e2] [invalid=true] description=\"Enter a valid address\""
        );
        assert_eq!(snap.refs.len(), 2);
    }

    #[test]
    fn shows_link_addresses_only_when_needed() {
        let tree = json!({"nodes": [
            {"nodeId": "1", "role": {"value": "RootWebArea"}, "childIds": ["2", "3"]},
            {"nodeId": "2", "role": {"value": "link"}, "name": {"value": "Docs"}, "backendDOMNodeId": 4,
             "properties": [{"name": "url", "value": {"value": "https://x.test/docs"}}], "childIds": []},
            {"nodeId": "3", "role": {"value": "link"}, "backendDOMNodeId": 5,
             "properties": [{"name": "url", "value": {"value": "https://x.test/icon"}}], "childIds": []}
        ]});
        let full = build(&tree, None).text;
        assert!(!full.contains("x.test/docs"));
        assert!(full.contains("-> https://x.test/icon"));
        assert!(build(&tree, Some("docs")).text.contains("-> https://x.test/docs"));
    }
}
