#[cfg(unix)]
mod bridge;
mod cdp;
#[cfg(unix)]
mod host;
mod snapshot;
#[cfg(not(unix))]
mod unsupported;
#[cfg(not(unix))]
use unsupported as bridge;
#[cfg(not(unix))]
use unsupported as host;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Result};
use serde_json::{json, Value};

use cdp::Cdp;
use snapshot::NodeRef;

pub use host::{install_native_host, run_host, run_native_host};

const BLOCKER_CHECK: &str = "function(other) { \
    const within = (inner, outer) => { for (let n = inner; n; n = n.parentNode || n.host) { if (n === outer) return true; } return false; }; \
    if (within(other, this) || within(this, other) || this.ownerDocument !== other.ownerDocument) return ''; \
    const el = other.nodeType === 1 ? other : other.parentElement; \
    if (!el) return 'another element'; \
    const label = el.getAttribute('aria-label') || el.id || (typeof el.className === 'string' ? el.className.split(' ')[0] : ''); \
    return el.tagName.toLowerCase() + (label ? ' ' + String(label).slice(0, 60) : ''); }";

const CHOICE_ROLES: &[&str] = &["option", "menuitem", "menuitemradio", "menuitemcheckbox", "treeitem", "radio", "tab"];

fn collect_child_frames(frame: &Value, found: &mut Vec<String>) {
    for child in frame["childFrames"].as_array().into_iter().flatten() {
        if let Some(id) = child["frame"]["id"].as_str() {
            found.push(id.to_string());
        }
        collect_child_frames(child, found);
    }
}

fn is_current(event: &Value, current: &std::sync::Mutex<String>) -> bool {
    current.lock().is_ok_and(|session| event["sessionId"] == session.as_str())
}

fn track_child_frames(mut events: tokio::sync::broadcast::Receiver<Value>, children: Arc<std::sync::Mutex<Vec<ChildFrame>>>) {
    tokio::spawn(async move {
        loop {
            let event = match events.recv().await {
                Ok(event) => event,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            };
            let Some(session) = event["sessionId"].as_str().filter(|s| !s.contains(':')) else { continue };
            let params = &event["params"];
            match event["method"].as_str() {
                Some("Target.attachedToTarget") if params["targetInfo"]["type"] == "iframe" => {
                    let (Some(raw), Some(frame_id)) =
                        (params["sessionId"].as_str(), params["targetInfo"]["targetId"].as_str())
                    else {
                        continue;
                    };
                    if let Ok(mut list) = children.lock() {
                        list.push(ChildFrame {
                            raw_session: raw.to_string(),
                            session: format!("{session}:{raw}"),
                            frame_id: frame_id.to_string(),
                        });
                    }
                }
                Some("Target.detachedFromTarget") => {
                    if let (Some(raw), Ok(mut list)) = (params["sessionId"].as_str(), children.lock()) {
                        list.retain(|child| child.raw_session != raw);
                    }
                }
                _ => {}
            }
        }
    });
}

fn splice_frame(nodes: &mut Vec<Value>, inner: &Value, prefix: &str, owner_id: i64, session: Option<&str>) {
    let Some(inner_nodes) = inner["nodes"].as_array().filter(|list| !list.is_empty()) else { return };
    let prefixed = |id: &Value| id.as_str().map(|s| json!(format!("{prefix}{s}")));
    let root = prefixed(&inner_nodes[0]["nodeId"]);
    let mut added = Vec::with_capacity(inner_nodes.len());
    for node in inner_nodes {
        let mut node = node.clone();
        if let Some(id) = prefixed(&node["nodeId"]) {
            node["nodeId"] = id;
        }
        if let Some(id) = prefixed(&node["parentId"]) {
            node["parentId"] = id;
        }
        if let Some(ids) = node["childIds"].as_array() {
            node["childIds"] = Value::Array(ids.iter().filter_map(prefixed).collect());
        }
        if let Some(session) = session {
            node["corexSession"] = json!(session);
            node["corexOwner"] = json!(owner_id);
        }
        added.push(node);
    }
    let (Some(root), Some(holder)) = (root, nodes.iter_mut().find(|n| n["backendDOMNodeId"].as_i64() == Some(owner_id)))
    else {
        return;
    };
    match holder["childIds"].as_array_mut() {
        Some(children) => children.push(root),
        None => holder["childIds"] = json!([root]),
    }
    nodes.extend(added);
}

fn quoted_name(line: &str) -> &str {
    let entry = line.trim_start().trim_start_matches("- ");
    let after_role = entry.split_once(' ').map_or("", |(_, rest)| rest);
    after_role.strip_prefix('"').and_then(|rest| rest.split('"').next()).unwrap_or("")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gesture {
    Click,
    Double,
    Right,
    Hover,
}

const MAX_CONSOLE: usize = 200;

const DOM_QUIET: &str = "new Promise(done => { \
    const finish = () => { observer.disconnect(); clearTimeout(cap); done(true); }; \
    let idle = setTimeout(finish, 120); \
    const structural = list => list.some(m => [...m.addedNodes, ...m.removedNodes].some(n => n.nodeType === 1)); \
    const observer = new MutationObserver(list => { if (structural(list)) { clearTimeout(idle); idle = setTimeout(finish, 120); } }); \
    observer.observe(document, { subtree: true, childList: true }); \
    const cap = setTimeout(finish, 1000); })";

const TEXT_WATCH: &str = "(needle, gone, ms) => new Promise(done => { \
    const visible = () => { \
        if ((document.body ? document.body.innerText : '').toLowerCase().includes(needle)) return true; \
        for (const e of document.querySelectorAll('input, textarea, [placeholder], [aria-label], [title]')) { \
            const said = [e.value, e.getAttribute('placeholder'), e.getAttribute('aria-label'), e.getAttribute('title')]; \
            if (said.some(v => typeof v === 'string' && v.toLowerCase().includes(needle))) return true; \
        } \
        return false; }; \
    const finish = ok => { clearInterval(poll); clearTimeout(cap); done(ok); }; \
    const check = () => { if (visible() !== gone) finish(true); }; \
    const poll = setInterval(check, 100); \
    const cap = setTimeout(() => finish(false), ms); \
    check(); })";
const LOAD_TIMEOUT: Duration = Duration::from_secs(20);

pub struct Screenshot {
    pub base64: String,
    pub media_type: &'static str,
}

#[derive(Clone)]
struct ChildFrame {
    raw_session: String,
    session: String,
    frame_id: String,
}

pub struct Browser {
    cdp: Arc<Cdp>,
    session: String,
    current: Arc<std::sync::Mutex<String>>,
    refs: HashMap<String, NodeRef>,
    console: Arc<std::sync::Mutex<Vec<String>>>,
    dialog: Arc<std::sync::Mutex<Option<String>>>,
    children: Arc<std::sync::Mutex<Vec<ChildFrame>>>,
    tabs: Vec<i64>,
    tab_events: Arc<std::sync::Mutex<TabEvents>>,
}

#[derive(Default)]
struct TabEvents {
    opened: Vec<i64>,
    closed: Vec<i64>,
}

pub struct TabInfo {
    pub id: i64,
    pub title: String,
    pub url: String,
    pub active: bool,
}

fn watch_tabs(mut events: tokio::sync::broadcast::Receiver<Value>, tab_events: Arc<std::sync::Mutex<TabEvents>>) {
    tokio::spawn(async move {
        loop {
            let event = match events.recv().await {
                Ok(event) => event,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            };
            let Some(tab) = event["params"]["tabId"].as_i64() else { continue };
            let Ok(mut state) = tab_events.lock() else { continue };
            match event["method"].as_str() {
                Some("Corex.tabOpened") => state.opened.push(tab),
                Some("Corex.tabClosed") => state.closed.push(tab),
                _ => {}
            }
        }
    });
}

impl Browser {
    pub async fn start() -> Result<Self> {
        Self::attach_extension(bridge::connect().await?).await
    }

    async fn attach_extension(cdp: Arc<Cdp>) -> Result<Self> {
        let opened = cdp.call(None, "Corex.openTab", json!({ "url": "about:blank" })).await?;
        let tab = opened["tabId"].as_i64().ok_or_else(|| anyhow!("the extension did not return a tab id"))?;
        let session = tab.to_string();
        let current = Arc::new(std::sync::Mutex::new(session.clone()));
        let console = Arc::new(std::sync::Mutex::new(Vec::new()));
        let dialog = Arc::new(std::sync::Mutex::new(None));
        collect_console(cdp.subscribe(), current.clone(), console.clone(), dialog.clone());
        let children = Arc::new(std::sync::Mutex::new(Vec::new()));
        track_child_frames(cdp.subscribe(), children.clone());
        let tab_events = Arc::default();
        watch_tabs(cdp.subscribe(), Arc::clone(&tab_events));
        let browser = Self {
            cdp,
            session,
            current,
            refs: HashMap::new(),
            console,
            dialog,
            children,
            tabs: vec![tab],
            tab_events,
        };
        browser.prepare_tab().await?;
        Ok(browser)
    }

    async fn prepare_tab(&self) -> Result<()> {
        for domain in ["Page", "Runtime", "DOM", "Accessibility"] {
            self.send(&format!("{domain}.enable"), json!({})).await?;
        }
        let _ = self
            .send(
                "Target.setAutoAttach",
                json!({ "autoAttach": true, "waitForDebuggerOnStart": false, "flatten": true }),
            )
            .await;
        Ok(())
    }

    fn current_tab(&self) -> i64 {
        self.session.parse().unwrap_or_default()
    }

    async fn switch_to(&mut self, tab: i64) -> Result<()> {
        self.session = tab.to_string();
        if let Ok(mut current) = self.current.lock() {
            current.clone_from(&self.session);
        }
        self.refs.clear();
        if let Ok(mut open) = self.dialog.lock() {
            *open = None;
        }
        let _ = self.cdp.call(None, "Corex.activateTab", json!({ "tabId": tab })).await;
        self.prepare_tab().await?;
        self.wait_ready().await;
        Ok(())
    }

    async fn wait_ready(&self) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while tokio::time::Instant::now() < deadline {
            if self.evaluate("document.readyState").await.ok().as_ref().and_then(Value::as_str) == Some("complete") {
                return;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    pub async fn sync_tabs(&mut self) -> Result<Option<String>> {
        let (opened, closed) = match self.tab_events.lock() {
            Ok(mut state) => (std::mem::take(&mut state.opened), std::mem::take(&mut state.closed)),
            Err(_) => return Ok(None),
        };
        for tab in &opened {
            if !self.tabs.contains(tab) {
                self.tabs.push(*tab);
            }
        }
        self.tabs.retain(|tab| !closed.contains(tab));
        if let Some(&newest) = opened.iter().rev().find(|tab| self.tabs.contains(tab)) {
            if newest != self.current_tab() {
                self.switch_to(newest).await?;
                return Ok(Some(format!(
                    "The page opened a new tab ({}), which is now the active tab. Use the tabs action to go back.",
                    self.page_info().await
                )));
            }
        }
        if self.tabs.contains(&self.current_tab()) {
            return Ok(None);
        }
        let Some(&previous) = self.tabs.last() else {
            return Err(anyhow!("the Corex tab was closed"));
        };
        self.switch_to(previous).await?;
        Ok(Some(format!("The active tab was closed; back on {}.", self.page_info().await)))
    }

    pub async fn list_tabs(&self) -> Vec<TabInfo> {
        let mut listed = Vec::new();
        for &id in &self.tabs {
            let info = self
                .cdp
                .call(
                    Some(&id.to_string()),
                    "Runtime.evaluate",
                    json!({ "expression": "JSON.stringify({url: location.href, title: document.title})", "returnByValue": true }),
                )
                .await
                .ok()
                .and_then(|v| v["result"]["value"].as_str().and_then(|s| serde_json::from_str::<Value>(s).ok()))
                .unwrap_or(Value::Null);
            listed.push(TabInfo {
                id,
                title: info["title"].as_str().unwrap_or("").to_string(),
                url: info["url"].as_str().unwrap_or("").to_string(),
                active: id == self.current_tab(),
            });
        }
        listed
    }

    pub async fn switch_tab(&mut self, tab: i64) -> Result<()> {
        if !self.tabs.contains(&tab) {
            return Err(anyhow!("tab {tab} is not one of the Corex tabs; use the tabs action to list them"));
        }
        self.switch_to(tab).await
    }

    pub async fn close_tab(&mut self, tab: i64) -> Result<()> {
        if !self.tabs.contains(&tab) {
            return Err(anyhow!("tab {tab} is not one of the Corex tabs; use the tabs action to list them"));
        }
        if self.tabs.len() == 1 {
            return Err(anyhow!("that is the last Corex tab; use the close action to end the browser session"));
        }
        self.cdp.call(None, "Corex.closeTab", json!({ "tabId": tab })).await?;
        self.tabs.retain(|open| *open != tab);
        if tab == self.current_tab() {
            let previous = *self.tabs.last().expect("another tab remains");
            self.switch_to(previous).await?;
        }
        Ok(())
    }

    pub fn open_dialog(&self) -> Option<String> {
        self.dialog.lock().ok().and_then(|open| open.clone())
    }

    pub fn is_alive(&self) -> bool {
        self.cdp.is_alive()
    }

    async fn send(&self, method: &str, params: Value) -> Result<Value> {
        self.cdp.call(Some(&self.session), method, params).await
    }

    // Chrome holds input events until an open JavaScript dialog is answered, so stop waiting once one opens.
    async fn send_input(&self, method: &str, params: Value) -> Result<bool> {
        let mut events = self.cdp.subscribe();
        let session = self.session.clone();
        let dialog = self.dialog.clone();
        let dialog_opened = async move {
            loop {
                match events.recv().await {
                    Ok(event)
                        if event["method"] == "Page.javascriptDialogOpening" && event["sessionId"] == session.as_str() =>
                    {
                        if let Ok(mut open) = dialog.lock() {
                            *open = Some(describe_dialog(&event["params"]));
                        }
                        return;
                    }
                    Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                    Err(_) => std::future::pending::<()>().await,
                }
            }
        };
        tokio::select! {
            result = self.send(method, params) => result.map(|_| false),
            () = dialog_opened => Ok(true),
        }
    }

    async fn evaluate(&self, expression: &str) -> Result<Value> {
        let result = self
            .send(
                "Runtime.evaluate",
                json!({ "expression": expression, "returnByValue": true, "awaitPromise": true }),
            )
            .await?;
        if let Some(details) = result.get("exceptionDetails") {
            return Err(anyhow!(
                "{}",
                details["exception"]["description"].as_str().unwrap_or("script error")
            ));
        }
        Ok(result["result"]["value"].clone())
    }

    pub async fn page_info(&self) -> String {
        let info = self
            .evaluate("JSON.stringify({url: location.href, title: document.title})")
            .await
            .ok()
            .and_then(|v| v.as_str().and_then(|s| serde_json::from_str::<Value>(s).ok()))
            .unwrap_or(Value::Null);
        format!(
            "Page: {} ({})",
            info["title"].as_str().unwrap_or(""),
            info["url"].as_str().unwrap_or("unknown")
        )
    }

    // The DOM is usable at DOMContentLoaded; the load event also waits for images, ads and trackers.
    async fn wait_for_load(&self, mut events: tokio::sync::broadcast::Receiver<Value>) {
        let session = self.session.clone();
        let _ = tokio::time::timeout(LOAD_TIMEOUT, async move {
            while let Ok(event) = events.recv().await {
                let ready = event["method"] == "Page.domContentEventFired" || event["method"] == "Page.loadEventFired";
                if ready && event["sessionId"] == session.as_str() {
                    break;
                }
            }
        })
        .await;
        self.wait_quiet().await;
    }

    async fn wait_quiet(&self) {
        if self.open_dialog().is_some() {
            return;
        }
        let _ = tokio::time::timeout(Duration::from_millis(1500), self.evaluate(DOM_QUIET)).await;
    }

    pub async fn navigate(&mut self, url: &str) -> Result<()> {
        let events = self.cdp.subscribe();
        let result = self.send("Page.navigate", json!({ "url": url })).await?;
        if let Some(error) = result["errorText"].as_str() {
            return Err(anyhow!("navigation failed: {error}"));
        }
        self.wait_for_load(events).await;
        Ok(())
    }

    pub async fn back(&mut self) -> Result<()> {
        let history = self.send("Page.getNavigationHistory", json!({})).await?;
        let index = history["currentIndex"].as_i64().unwrap_or(0);
        if index <= 0 {
            return Err(anyhow!("no earlier page in the history"));
        }
        let entry = history["entries"][(index - 1) as usize]["id"].clone();
        let events = self.cdp.subscribe();
        self.send("Page.navigateToHistoryEntry", json!({ "entryId": entry })).await?;
        self.wait_for_load(events).await;
        Ok(())
    }

    async fn accessibility_tree(&self) -> Result<Value> {
        let main = self.send("Accessibility.getFullAXTree", json!({})).await?;
        let mut child_frames = Vec::new();
        if let Ok(frames) = self.send("Page.getFrameTree", json!({})).await {
            collect_child_frames(&frames["frameTree"], &mut child_frames);
        }
        let mut nodes = main["nodes"].as_array().cloned().unwrap_or_default();
        let prefix = format!("{}:", self.session);
        let separate_processes: Vec<ChildFrame> = self
            .children
            .lock()
            .map(|list| list.iter().filter(|child| child.session.starts_with(&prefix)).cloned().collect())
            .unwrap_or_default();
        for (index, frame_id) in child_frames.iter().enumerate() {
            if separate_processes.iter().any(|child| &child.frame_id == frame_id) {
                continue;
            }
            let Ok(owner) = self.send("DOM.getFrameOwner", json!({ "frameId": frame_id })).await else { continue };
            let Some(owner_id) = owner["backendNodeId"].as_i64() else { continue };
            let Ok(inner) = self.send("Accessibility.getFullAXTree", json!({ "frameId": frame_id })).await else { continue };
            splice_frame(&mut nodes, &inner, &format!("f{index}-"), owner_id, None);
        }
        for (index, child) in separate_processes.iter().enumerate() {
            for domain in ["DOM", "Accessibility"] {
                let _ = self.cdp.call(Some(&child.session), &format!("{domain}.enable"), json!({})).await;
            }
            let Ok(owner) = self.send("DOM.getFrameOwner", json!({ "frameId": child.frame_id })).await else { continue };
            let Some(owner_id) = owner["backendNodeId"].as_i64() else { continue };
            let Ok(inner) = self.cdp.call(Some(&child.session), "Accessibility.getFullAXTree", json!({})).await else {
                continue;
            };
            splice_frame(&mut nodes, &inner, &format!("o{index}-"), owner_id, Some(&child.session));
        }
        Ok(json!({ "nodes": nodes }))
    }

    pub async fn snapshot(&mut self) -> Result<String> {
        self.snapshot_matching(None).await
    }

    pub async fn snapshot_matching(&mut self, query: Option<&str>) -> Result<String> {
        if let Some(dialog) = self.open_dialog() {
            return Err(anyhow!("a JavaScript dialog is open ({dialog}); answer it with the dialog action first"));
        }
        let tree = self.accessibility_tree().await?;
        let snap = snapshot::build(&tree, query);
        self.refs = snap.refs;
        let text = if !snap.text.trim().is_empty() {
            snap.text
        } else if let Some(query) = query {
            format!("(nothing on the page matches '{query}')\n")
        } else {
            "(the page has no accessible content yet)\n".to_string()
        };
        Ok(format!("{}\n{}", self.page_info().await, text))
    }

    fn node(&self, reference: &str) -> Result<NodeRef> {
        self.refs.get(reference).cloned().ok_or_else(|| {
            anyhow!("unknown ref '{reference}'. Refs belong to the latest snapshot; take a new snapshot first")
        })
    }

    async fn send_in(&self, node: &NodeRef, method: &str, params: Value) -> Result<Value> {
        self.cdp.call(Some(node.session.as_deref().unwrap_or(&self.session)), method, params).await
    }

    async fn scroll_into_view(&self, node: &NodeRef) {
        if let Some(owner) = node.owner {
            let _ = self.send("DOM.scrollIntoViewIfNeeded", json!({ "backendNodeId": owner })).await;
        }
        let _ = self.send_in(node, "DOM.scrollIntoViewIfNeeded", json!({ "backendNodeId": node.backend })).await;
    }

    async fn run_on_node(&self, node: &NodeRef, function: &str) -> Result<()> {
        let resolved = self.send_in(node, "DOM.resolveNode", json!({ "backendNodeId": node.backend })).await?;
        let object_id = resolved["object"]["objectId"]
            .as_str()
            .ok_or_else(|| anyhow!("the element is no longer on the page"))?;
        self.send_in(
            node,
            "Runtime.callFunctionOn",
            json!({ "objectId": object_id, "functionDeclaration": function }),
        )
        .await?;
        Ok(())
    }

    async fn center_of(&self, node: &NodeRef) -> Option<(f64, f64)> {
        let model = self.send_in(node, "DOM.getBoxModel", json!({ "backendNodeId": node.backend })).await.ok()?;
        let points: Vec<f64> = model["model"]["content"].as_array()?.iter().filter_map(Value::as_f64).collect();
        if points.len() != 8 {
            return None;
        }
        let (mut x, mut y) = (
            points.iter().step_by(2).sum::<f64>() / 4.0,
            points.iter().skip(1).step_by(2).sum::<f64>() / 4.0,
        );
        if let Some(owner) = node.owner {
            let frame = self.send("DOM.getBoxModel", json!({ "backendNodeId": owner })).await.ok()?;
            let origin: Vec<f64> = frame["model"]["content"].as_array()?.iter().filter_map(Value::as_f64).collect();
            x += origin.first()?;
            y += origin.get(1)?;
        }
        Some((x, y))
    }

    async fn blocker_at(&self, node: &NodeRef, x: f64, y: f64) -> Option<String> {
        if node.session.is_some() {
            return None;
        }
        let backend_id = node.backend;
        let hit = self
            .send("DOM.getNodeForLocation", json!({ "x": x.round() as i64, "y": y.round() as i64, "includeUserAgentShadowDOM": true }))
            .await
            .ok()?;
        let hit_id = hit["backendNodeId"].as_i64()?;
        if hit_id == backend_id {
            return None;
        }
        let object_of = |resolved: Value| resolved["object"]["objectId"].as_str().map(str::to_string);
        let target = object_of(self.send("DOM.resolveNode", json!({ "backendNodeId": backend_id })).await.ok()?)?;
        let other = object_of(self.send("DOM.resolveNode", json!({ "backendNodeId": hit_id })).await.ok()?)?;
        let verdict = self
            .send(
                "Runtime.callFunctionOn",
                json!({
                    "objectId": target,
                    "functionDeclaration": BLOCKER_CHECK,
                    "arguments": [{ "objectId": other }],
                    "returnByValue": true
                }),
            )
            .await
            .ok()?;
        verdict["result"]["value"].as_str().filter(|text| !text.is_empty()).map(str::to_string)
    }

    async fn point_at(&self, node: &NodeRef) {
        if let Some((x, y)) = self.center_of(node).await {
            let _ = self.send("Corex.pointer", json!({ "x": x, "y": y, "press": true })).await;
        }
    }

    pub async fn click(&mut self, reference: &str) -> Result<()> {
        self.pointer(reference, Gesture::Click).await
    }

    pub async fn pointer(&mut self, reference: &str, gesture: Gesture) -> Result<()> {
        let node = self.node(reference)?;
        self.scroll_into_view(&node).await;
        let events = self.cdp.subscribe();

        let Some((mut x, mut y)) = self.center_of(&node).await else {
            if gesture == Gesture::Click {
                self.run_on_node(&node, "function() { this.click(); }").await?;
                self.settle(events).await;
                return Ok(());
            }
            return Err(anyhow!("{reference} has no visible position on the page"));
        };

        if gesture != Gesture::Hover && self.blocker_at(&node, x, y).await.is_some() {
            let _ = self
                .run_on_node(&node, "function() { this.scrollIntoView({ block: 'center', inline: 'center' }); }")
                .await;
            tokio::time::sleep(Duration::from_millis(200)).await;
            if let Some(moved) = self.center_of(&node).await {
                (x, y) = moved;
            }
            if let Some(blocker) = self.blocker_at(&node, x, y).await {
                return Err(anyhow!(
                    "{reference} is covered by <{blocker}>, so a click would not reach it. \
                     Close the overlay, dialog or menu in front of it first"
                ));
            }
        }

        let (button, buttons, presses): (&str, u32, &[u32]) = match gesture {
            Gesture::Hover => ("none", 0, &[]),
            Gesture::Click => ("left", 1, &[1]),
            Gesture::Double => ("left", 1, &[1, 2]),
            Gesture::Right => ("right", 2, &[1]),
        };
        let mut blocked = self
            .send_input(
                "Input.dispatchMouseEvent",
                json!({ "type": "mouseMoved", "x": x, "y": y, "button": "none", "buttons": 0 }),
            )
            .await?;
        'presses: for count in presses {
            for (kind, held) in [("mousePressed", buttons), ("mouseReleased", 0)] {
                if blocked {
                    break 'presses;
                }
                blocked = self
                    .send_input(
                        "Input.dispatchMouseEvent",
                        json!({ "type": kind, "x": x, "y": y, "button": button, "buttons": held, "clickCount": count }),
                    )
                    .await?;
            }
        }
        if !blocked {
            self.settle(events).await;
        }
        Ok(())
    }

    /// Opens a custom dropdown or menu, finds the entry by its visible text and clicks it.
    pub async fn choose(&mut self, reference: &str, wanted: &str) -> Result<()> {
        self.click(reference).await?;
        tokio::time::sleep(Duration::from_millis(150)).await;
        let snapshot = self.snapshot().await?;
        let needle = wanted.to_lowercase();
        let entries: Vec<(&str, &str)> = snapshot
            .lines()
            .filter(|line| CHOICE_ROLES.iter().any(|role| line.trim_start().starts_with(&format!("- {role} "))))
            .filter_map(|line| Some((line, line.split("[ref=").nth(1)?.split(']').next()?)))
            .filter(|(_, found)| *found != reference)
            .collect();
        let hit = entries
            .iter()
            .find(|(line, _)| quoted_name(line).to_lowercase() == needle)
            .or_else(|| entries.iter().find(|(line, _)| quoted_name(line).to_lowercase().contains(&needle)));
        match hit {
            Some((_, found)) => {
                let found = found.to_string();
                self.click(&found).await
            }
            None => {
                let seen: Vec<String> = entries.iter().take(20).map(|(line, _)| line.trim().to_string()).collect();
                Err(anyhow!(
                    "no option matching '{wanted}' after opening {reference}. Options visible: {}",
                    if seen.is_empty() { "none (take a snapshot or a screenshot)".to_string() } else { seen.join(" | ") }
                ))
            }
        }
    }

    pub async fn upload(&mut self, reference: &str, paths: &[String]) -> Result<()> {
        let node = self.node(reference)?;
        for path in paths {
            if !std::path::Path::new(path).is_file() {
                return Err(anyhow!("'{path}' is not a file on this machine"));
            }
        }
        self.send_in(&node, "DOM.setFileInputFiles", json!({ "backendNodeId": node.backend, "files": paths })).await?;
        Ok(())
    }

    pub async fn dialog(&self, accept: bool, text: Option<&str>) -> Result<()> {
        let mut params = json!({ "accept": accept });
        if let Some(text) = text {
            params["promptText"] = json!(text);
        }
        self.send("Page.handleJavaScriptDialog", params).await?;
        Ok(())
    }

    async fn settle(&self, events: tokio::sync::broadcast::Receiver<Value>) {
        let session = self.session.clone();
        let mut events = events;
        let navigated = tokio::time::timeout(Duration::from_millis(250), async move {
            while let Ok(event) = events.recv().await {
                if event["method"] == "Page.frameStartedLoading" && event["sessionId"] == session.as_str() {
                    return true;
                }
            }
            false
        })
        .await
        .unwrap_or(false);
        if navigated {
            let events = self.cdp.subscribe();
            self.wait_for_load(events).await;
        } else {
            self.wait_quiet().await;
        }
    }

    pub async fn type_text(&mut self, reference: &str, text: &str, clear: bool, submit: bool) -> Result<()> {
        let node = self.node(reference)?;
        self.scroll_into_view(&node).await;
        self.point_at(&node).await;
        self.send_in(&node, "DOM.focus", json!({ "backendNodeId": node.backend })).await?;
        if clear {
            self.run_on_node(
                &node,
                "function() { this.focus(); if (this.select) { this.select(); } else { \
                 const r = document.createRange(); r.selectNodeContents(this); \
                 const s = getSelection(); s.removeAllRanges(); s.addRange(r); } }",
            )
            .await?;
        }
        self.send_in(&node, "Input.insertText", json!({ "text": text })).await?;
        if submit {
            self.press_key("Enter").await?;
        }
        Ok(())
    }

    pub async fn select_option(&mut self, reference: &str, value: &str) -> Result<()> {
        let node = self.node(reference)?;
        self.scroll_into_view(&node).await;
        self.point_at(&node).await;
        let literal = Value::String(value.to_string()).to_string();
        let function = format!(
            "function() {{ const wanted = {literal}; \
             const option = [...this.options].find(o => o.value === wanted || o.text.trim() === wanted); \
             if (!option) throw new Error('option not found: ' + wanted); \
             this.value = option.value; \
             this.dispatchEvent(new Event('input', {{bubbles: true}})); \
             this.dispatchEvent(new Event('change', {{bubbles: true}})); }}"
        );
        self.run_on_node(&node, &function).await
    }

    pub async fn press_key(&self, key: &str) -> Result<()> {
        let Key { name, code, text, vk, modifiers, command } =
            parse_key(key).ok_or_else(|| anyhow!("unsupported key '{key}'. Supported: {SUPPORTED_KEYS}"))?;
        let events = self.cdp.subscribe();
        let base = json!({ "key": name, "code": code, "windowsVirtualKeyCode": vk, "modifiers": modifiers });
        let mut down = base.clone();
        down["type"] = json!("rawKeyDown");
        if let Some(text) = text {
            down["type"] = json!("keyDown");
            down["text"] = json!(text);
        }
        if let Some(command) = command {
            down["commands"] = json!([command]);
        }
        if self.send_input("Input.dispatchKeyEvent", down).await? {
            return Ok(());
        }
        let mut up = base;
        up["type"] = json!("keyUp");
        if !self.send_input("Input.dispatchKeyEvent", up).await? {
            self.settle(events).await;
        }
        Ok(())
    }

    pub async fn scroll(&self, direction: &str, reference: Option<&str>) -> Result<()> {
        let sign = match direction {
            "up" => -1.0,
            "down" => 1.0,
            other => return Err(anyhow!("direction must be 'up' or 'down', got '{other}'")),
        };
        let viewport = self.evaluate("[window.innerWidth, window.innerHeight]").await?;
        let width = viewport[0].as_f64().unwrap_or(1280.0);
        let height = viewport[1].as_f64().unwrap_or(800.0);
        let (x, y) = match reference {
            Some(reference) => {
                let node = self.node(reference)?;
                self.center_of(&node)
                    .await
                    .ok_or_else(|| anyhow!("{reference} has no visible position on the page"))?
            }
            None => (width / 2.0, height / 2.0),
        };
        self.send_input(
            "Input.dispatchMouseEvent",
            json!({ "type": "mouseWheel", "x": x, "y": y, "deltaX": 0, "deltaY": sign * height * 0.85 }),
        )
        .await?;
        tokio::time::sleep(Duration::from_millis(350)).await;
        Ok(())
    }

    pub async fn wait_for_text(&self, text: &str, gone: bool, timeout: Duration) -> Result<bool> {
        let needle = Value::String(text.to_lowercase()).to_string();
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now()).min(Duration::from_secs(25));
            let expression = format!("({TEXT_WATCH})({needle}, {gone}, {})", left.as_millis());
            if self.evaluate(&expression).await.ok().and_then(|v| v.as_bool()) == Some(true) {
                return Ok(true);
            }
            if tokio::time::Instant::now() >= deadline {
                return Ok(false);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    pub async fn page_text(&self) -> Result<String> {
        let value = self.evaluate("document.body ? document.body.innerText : ''").await?;
        Ok(value.as_str().unwrap_or("").to_string())
    }

    pub async fn screenshot(&self) -> Result<Screenshot> {
        let shot = self
            .send("Page.captureScreenshot", json!({ "format": "jpeg", "quality": 70 }))
            .await?;
        let base64 = shot["data"].as_str().ok_or_else(|| anyhow!("empty screenshot"))?.to_string();
        Ok(Screenshot { base64, media_type: "image/jpeg" })
    }

    pub fn take_console(&self) -> Vec<String> {
        self.console
            .lock()
            .map(|mut lines| std::mem::take(&mut *lines))
            .unwrap_or_default()
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        for tab in &self.tabs {
            self.cdp.notify("Corex.closeTab", json!({ "tabId": tab }));
        }
    }
}

fn collect_console(
    mut events: tokio::sync::broadcast::Receiver<Value>,
    current: Arc<std::sync::Mutex<String>>,
    sink: Arc<std::sync::Mutex<Vec<String>>>,
    dialog: Arc<std::sync::Mutex<Option<String>>>,
) {
    tokio::spawn(async move {
        loop {
            let event = match events.recv().await {
                Ok(event) => event,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            };
            if !is_current(&event, &current) {
                continue;
            }
            let line = match event["method"].as_str() {
                Some("Runtime.consoleAPICalled") => {
                    let parts: Vec<String> = event["params"]["args"]
                        .as_array()
                        .map(|args| {
                            args.iter()
                                .map(|a| {
                                    a["value"]
                                        .as_str()
                                        .map(str::to_string)
                                        .or_else(|| a["description"].as_str().map(str::to_string))
                                        .unwrap_or_else(|| a["value"].to_string())
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    format!("[{}] {}", event["params"]["type"].as_str().unwrap_or("log"), parts.join(" "))
                }
                Some("Runtime.exceptionThrown") => format!(
                    "[exception] {}",
                    event["params"]["exceptionDetails"]["exception"]["description"]
                        .as_str()
                        .or_else(|| event["params"]["exceptionDetails"]["text"].as_str())
                        .unwrap_or("uncaught error")
                ),
                Some("Page.javascriptDialogOpening") => {
                    let described = describe_dialog(&event["params"]);
                    if let Ok(mut open) = dialog.lock() {
                        *open = Some(described.clone());
                    }
                    format!("[dialog open] {described}")
                }
                Some("Page.javascriptDialogClosed") => {
                    if let Ok(mut open) = dialog.lock() {
                        *open = None;
                    }
                    continue;
                }
                _ => continue,
            };
            if let Ok(mut lines) = sink.lock() {
                if lines.len() >= MAX_CONSOLE {
                    lines.remove(0);
                }
                lines.push(line);
            }
        }
    });
}

fn describe_dialog(params: &Value) -> String {
    format!("{}: {}", params["type"].as_str().unwrap_or("alert"), params["message"].as_str().unwrap_or(""))
}

struct Key {
    name: String,
    code: String,
    text: Option<String>,
    vk: u32,
    modifiers: u32,
    command: Option<&'static str>,
}

const ALT: u32 = 1;
const CTRL: u32 = 2;
const META: u32 = 4;
const SHIFT: u32 = 8;

fn named_key(key: &str) -> Option<(&'static str, &'static str, Option<&'static str>, u32)> {
    Some(match key.to_ascii_lowercase().as_str() {
        "enter" | "return" => ("Enter", "Enter", Some("\r"), 13),
        "tab" => ("Tab", "Tab", None, 9),
        "escape" | "esc" => ("Escape", "Escape", None, 27),
        "backspace" => ("Backspace", "Backspace", None, 8),
        "delete" | "del" => ("Delete", "Delete", None, 46),
        "space" => (" ", "Space", Some(" "), 32),
        "arrowup" | "up" => ("ArrowUp", "ArrowUp", None, 38),
        "arrowdown" | "down" => ("ArrowDown", "ArrowDown", None, 40),
        "arrowleft" | "left" => ("ArrowLeft", "ArrowLeft", None, 37),
        "arrowright" | "right" => ("ArrowRight", "ArrowRight", None, 39),
        "pageup" => ("PageUp", "PageUp", None, 33),
        "pagedown" => ("PageDown", "PageDown", None, 34),
        "home" => ("Home", "Home", None, 36),
        "end" => ("End", "End", None, 35),
        _ => return None,
    })
}

fn parse_key(spec: &str) -> Option<Key> {
    let parts: Vec<&str> = spec.split('+').map(str::trim).collect();
    let (key, held) = match parts.split_last()? {
        (&"", rest) if !rest.is_empty() => ("+", &rest[..rest.len() - 1]),
        (key, rest) => (*key, rest),
    };
    let mut modifiers = 0;
    for modifier in held {
        modifiers |= match modifier.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => CTRL,
            "alt" | "option" => ALT,
            "shift" => SHIFT,
            "meta" | "cmd" | "command" | "super" => META,
            _ => return None,
        };
    }
    let chord = modifiers & (CTRL | ALT | META) != 0;
    if let Some((name, code, text, vk)) = named_key(key) {
        let text = text.filter(|_| !chord).map(str::to_string);
        return Some(Key { name: name.into(), code: code.into(), text, vk, modifiers, command: None });
    }
    if let Some(number) = key.strip_prefix(['F', 'f']).and_then(|n| n.parse::<u32>().ok()).filter(|n| (1..=12).contains(n)) {
        return Some(Key { name: format!("F{number}"), code: format!("F{number}"), text: None, vk: 111 + number, modifiers, command: None });
    }
    let mut chars = key.chars();
    let (Some(c), None) = (chars.next(), chars.next()) else { return None };
    let upper = c.to_ascii_uppercase();
    let (code, vk) = if c.is_ascii_alphabetic() {
        (format!("Key{upper}"), upper as u32)
    } else if c.is_ascii_digit() {
        (format!("Digit{c}"), c as u32)
    } else if c.is_ascii_graphic() {
        (String::new(), 0)
    } else {
        return None;
    };
    let typed = if modifiers & SHIFT != 0 { upper } else { c };
    let command = if cfg!(target_os = "macos") && modifiers & META != 0 {
        match (c.to_ascii_lowercase(), modifiers & SHIFT != 0) {
            ('a', false) => Some("selectAll"),
            ('c', false) => Some("copy"),
            ('x', false) => Some("cut"),
            ('v', false) => Some("paste"),
            ('z', false) => Some("undo"),
            ('z', true) => Some("redo"),
            _ => None,
        }
    } else {
        None
    };
    Some(Key {
        name: typed.to_string(),
        code,
        text: (!chord).then(|| typed.to_string()),
        vk,
        modifiers,
        command,
    })
}

pub const SUPPORTED_KEYS: &str = "Enter, Tab, Escape, Backspace, Delete, Space, ArrowUp, ArrowDown, ArrowLeft, ArrowRight, \
     PageUp, PageDown, Home, End, F1-F12, single characters, and combinations such as Control+A, Shift+Tab or Meta+S";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splices_a_frame_tree_under_its_owner_and_tags_separate_process_nodes() {
        let mut nodes = vec![
            json!({"nodeId": "1", "role": {"value": "RootWebArea"}, "childIds": ["2"]}),
            json!({"nodeId": "2", "role": {"value": "Iframe"}, "backendDOMNodeId": 15, "childIds": []}),
        ];
        let inner = json!({"nodes": [
            {"nodeId": "1", "role": {"value": "RootWebArea"}, "childIds": ["2"]},
            {"nodeId": "2", "parentId": "1", "role": {"value": "button"}, "backendDOMNodeId": 9, "childIds": []}
        ]});
        splice_frame(&mut nodes, &inner, "o0-", 15, Some("7:child"));
        assert_eq!(nodes[1]["childIds"], json!(["o0-1"]));
        assert_eq!(nodes[3]["nodeId"], "o0-2");
        assert_eq!(nodes[3]["parentId"], "o0-1");
        assert_eq!(nodes[3]["corexSession"], "7:child");
        assert_eq!(nodes[3]["corexOwner"], 15);
    }

    #[test]
    fn parses_key_combinations() {
        let select_all = parse_key("Control+a").unwrap();
        assert_eq!((select_all.code.as_str(), select_all.vk, select_all.modifiers), ("KeyA", 65, CTRL));
        assert!(select_all.text.is_none());
        let back = parse_key("shift+Tab").unwrap();
        assert_eq!((back.name.as_str(), back.modifiers), ("Tab", SHIFT));
        assert_eq!(parse_key("Shift+x").unwrap().text.as_deref(), Some("X"));
        assert_eq!(parse_key("Enter").unwrap().text.as_deref(), Some("\r"));
        assert_eq!(parse_key("F5").unwrap().vk, 116);
        assert_eq!(parse_key("Control++").unwrap().name, "+");
        assert!(parse_key("Hyper+a").is_none());
        assert!(parse_key("Banana").is_none());
    }

    #[test]
    fn leaves_the_tree_alone_when_the_frame_owner_is_unknown() {
        let mut nodes = vec![json!({"nodeId": "1", "role": {"value": "RootWebArea"}, "childIds": []})];
        let inner = json!({"nodes": [{"nodeId": "1", "role": {"value": "RootWebArea"}, "childIds": []}]});
        splice_frame(&mut nodes, &inner, "f0-", 99, None);
        assert_eq!(nodes.len(), 1);
    }
}
