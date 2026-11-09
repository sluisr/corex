const HOST = "com.corex.bridge";
let port = null;
let groupId = null;
const tabs = new Set();

function connect() {
  if (port) return;
  let next;
  try {
    next = chrome.runtime.connectNative(HOST);
  } catch (_) {
    return;
  }
  port = next;
  updateBadge(true);

  next.onMessage.addListener((msg) => handle(msg));
  next.onDisconnect.addListener(() => {
    void chrome.runtime.lastError;
    if (port !== next) return;
    port = null;
    updateBadge(false);
    for (const tabId of tabs) chrome.debugger.detach({ tabId }).catch(() => {});
    tabs.clear();
    setTimeout(connect, 3000);
  });
}

function updateBadge(on) {
  if (on) {
    chrome.action.setBadgeText({ text: "ON" });
    chrome.action.setBadgeBackgroundColor({ color: "#10b981" });
  } else {
    chrome.action.setBadgeText({ text: "" });
  }
}

function send(message) {
  if (port) port.postMessage(message);
}

function reply(id, result, error) {
  send(error ? { id, error: { message: String(error) } } : { id, result: result ?? {} });
}

const lastCursor = new Map();

const lastActive = new Map();
const ORANGE = "249,115,22";
const MOVE_MS = 340;
const PRESS_MS = 90;

const FRAME_JS = `
  let frame = document.getElementById("__corex_frame");
  if (!frame) {
    frame = document.createElement("div");
    frame.id = "__corex_frame";
    frame.setAttribute("aria-hidden", "true");
    frame.style.cssText = "position:fixed;inset:0;z-index:2147483645;pointer-events:none;opacity:0;" +
      "transition:opacity .3s;box-shadow:inset 0 0 0 3px rgba(${ORANGE},.9),inset 0 0 36px rgba(${ORANGE},.35)";
    document.documentElement.appendChild(frame);
    void frame.getBoundingClientRect();
  }
  frame.style.opacity = "1";
  clearTimeout(window.__corexFrameTimer);
  window.__corexFrameTimer = setTimeout(() => { frame.style.opacity = "0"; }, 6000);`;

const CURSOR_SCRIPT = (x, y, fromX, fromY, pressed) => `(() => {
  ${FRAME_JS}
  let cursor = document.getElementById("__corex_cursor");
  if (!cursor) {
    cursor = document.createElement("div");
    cursor.id = "__corex_cursor";
    cursor.setAttribute("aria-hidden", "true");
    cursor.style.cssText = "position:fixed;left:0;top:0;z-index:2147483647;pointer-events:none;opacity:0;" +
      "transition:transform .32s cubic-bezier(.25,.8,.25,1),opacity .2s;" +
      "filter:drop-shadow(0 2px 3px rgba(0,0,0,.35));transform:translate(${fromX - 4}px, ${fromY - 2}px)";
    const ns = "http://www.w3.org/2000/svg";
    const svg = document.createElementNS(ns, "svg");
    svg.setAttribute("width", "22");
    svg.setAttribute("height", "22");
    svg.setAttribute("viewBox", "0 0 28 28");
    const arrow = document.createElementNS(ns, "path");
    arrow.setAttribute("d", "M5 3v19.5l5.2-4.6 3.4 7.6 3.6-1.6-3.4-7.4H20.5z");
    arrow.setAttribute("fill", "rgb(${ORANGE})");
    arrow.setAttribute("stroke", "#fff");
    arrow.setAttribute("stroke-width", "1.5");
    arrow.setAttribute("stroke-linejoin", "round");
    svg.appendChild(arrow);
    const label = document.createElement("div");
    label.textContent = "Corex";
    label.style.cssText = "position:absolute;left:15px;top:19px;padding:2px 6px;border-radius:999px;" +
      "background:rgb(${ORANGE});color:#fff;font:600 9px/1.2 system-ui,-apple-system,sans-serif;" +
      "letter-spacing:.02em;white-space:nowrap;box-shadow:0 1px 0 rgba(255,255,255,.35) inset";
    cursor.appendChild(svg);
    cursor.appendChild(label);
    document.documentElement.appendChild(cursor);
    void cursor.getBoundingClientRect();
  }
  cursor.style.opacity = "1";
  cursor.style.transform = "translate(${x - 4}px, ${y - 2}px)";
  if (${pressed ? "true" : "false"}) {
    const ring = document.createElement("div");
    ring.setAttribute("aria-hidden", "true");
    ring.style.cssText = "position:fixed;left:${x - 12}px;top:${y - 12}px;width:24px;height:24px;" +
      "border-radius:50%;background:rgba(${ORANGE},.28);border:2px solid rgba(${ORANGE},.9);" +
      "z-index:2147483646;pointer-events:none;";
    document.documentElement.appendChild(ring);
    ring.animate(
      [{ transform: "scale(.4)", opacity: 1 }, { transform: "scale(1.8)", opacity: 0 }],
      { duration: 450, easing: "ease-out" }
    ).onfinish = () => ring.remove();
  }
  clearTimeout(window.__corexCursorTimer);
  window.__corexCursorTimer = setTimeout(() => { cursor.style.opacity = "0"; }, 6000);
})()`;

async function inPage(tabId, expression) {
  try {
    const out = await chrome.debugger.sendCommand({ tabId }, "Runtime.evaluate", { expression });
    if (out?.exceptionDetails) console.warn("Corex overlay:", out.exceptionDetails.exception?.description);
  } catch (e) {
    console.warn("Corex overlay:", e);
  }
}

async function moveCursor(tabId, x, y, pressed) {
  const before = lastCursor.get(tabId);
  const from = before ?? { x: Math.max(20, x - 180), y: Math.max(20, y - 120) };
  lastCursor.set(tabId, { x, y, at: Date.now() });
  lastActive.set(tabId, Date.now());
  await inPage(tabId, CURSOR_SCRIPT(x, y, from.x, from.y, pressed));
}

async function pulseFrame(tabId) {
  lastActive.set(tabId, Date.now());
  await inPage(tabId, `(() => {${FRAME_JS}})()`);
}

async function control(tabId) {
  await chrome.debugger.attach({ tabId }, "1.3");
  tabs.add(tabId);
  try {
    groupId = await chrome.tabs.group({ tabIds: [tabId], groupId: groupId ?? undefined });
    await chrome.tabGroups.update(groupId, { title: "Corex", color: "orange" });
  } catch (_) {
    groupId = null;
  }
}

const KEPT_PROPERTIES = new Set([
  "level", "checked", "pressed", "expanded", "selected", "disabled", "required", "readonly", "invalid", "focused", "modal", "url",
]);

function compactTree(result) {
  if (!Array.isArray(result?.nodes)) return result;
  const dropped = new Set();
  for (const node of result.nodes) if (node.role?.value === "InlineTextBox") dropped.add(node.nodeId);
  const nodes = [];
  for (const node of result.nodes) {
    if (dropped.has(node.nodeId)) continue;
    const kept = { nodeId: node.nodeId };
    if (node.parentId) kept.parentId = node.parentId;
    const children = node.childIds?.filter((id) => !dropped.has(id));
    if (children?.length) kept.childIds = children;
    if (node.backendDOMNodeId) kept.backendDOMNodeId = node.backendDOMNodeId;
    if (node.ignored) {
      kept.ignored = true;
      const hidden = node.ignoredReasons?.filter((reason) => reason.name?.startsWith("ariaHidden"));
      if (hidden?.length) kept.ignoredReasons = hidden.map((reason) => ({ name: reason.name }));
    }
    for (const key of ["role", "name", "value", "description"]) {
      const value = node[key]?.value;
      if (value !== undefined && value !== "") kept[key] = { value };
    }
    const properties = node.properties
      ?.filter((property) => KEPT_PROPERTIES.has(property.name))
      .map((property) => ({ name: property.name, value: { value: property.value?.value } }));
    if (properties?.length) kept.properties = properties;
    nodes.push(kept);
  }
  return { nodes };
}

async function handle(msg) {
  const { id, method, params, sessionId } = msg;
  try {
    if (method === "Corex.openTab") {
      const tab = await chrome.tabs.create({ url: params?.url || "about:blank", active: true });
      await control(tab.id);
      return reply(id, { tabId: tab.id });
    }

    if (method === "Corex.activateTab") {
      const tab = await chrome.tabs.update(params.tabId, { active: true });
      await chrome.windows.update(tab.windowId, { focused: true }).catch(() => {});
      return reply(id, {});
    }

    if (method === "Corex.closeTab") {
      const tabId = params?.tabId;
      if (tabId && tabs.has(tabId)) {
        tabs.delete(tabId);
        await chrome.debugger.detach({ tabId }).catch(() => {});
        await chrome.tabs.remove(tabId).catch(() => {});
      }
      return id === undefined ? undefined : reply(id, {});
    }

    const [tabPart, childSession] = String(sessionId).split(":");
    const tabId = Number(tabPart);
    if (!tabs.has(tabId)) throw new Error("Tab is not controlled by Corex");
    if (method === "Corex.pointer") {
      const before = lastCursor.get(tabId);
      const far = !before || Math.hypot(before.x - params.x, before.y - params.y) > 8;
      await moveCursor(tabId, params.x, params.y, params.press === true);
      await new Promise((done) => setTimeout(done, far ? MOVE_MS : PRESS_MS));
      return reply(id, {});
    }
    if (["Page.navigate", "Input.insertText", "Input.dispatchKeyEvent"].includes(method)) {
      await pulseFrame(tabId);
    }
    const aimed = method === "Input.dispatchMouseEvent" && Number.isFinite(params?.x) && Number.isFinite(params?.y);
    if (aimed && params.type !== "mouseReleased") {
      const before = lastCursor.get(tabId);
      const far = !before || Math.hypot(before.x - params.x, before.y - params.y) > 8;
      const pressed = params.type === "mousePressed";
      await moveCursor(tabId, params.x, params.y, pressed);
      await new Promise((done) => setTimeout(done, pressed ? PRESS_MS : far ? MOVE_MS : 0));
    }
    const target = childSession ? { tabId, sessionId: childSession } : { tabId };
    const result = await chrome.debugger.sendCommand(target, method, params || {});
    reply(id, method === "Accessibility.getFullAXTree" ? compactTree(result) : result);
  } catch (e) {
    if (id !== undefined) reply(id, null, e.message || e);
  }
}

chrome.debugger.onEvent.addListener((source, method, params) => {
  if (!tabs.has(source.tabId)) return;
  send({ method, params, sessionId: source.sessionId ? `${source.tabId}:${source.sessionId}` : String(source.tabId) });
  if (source.sessionId || method !== "Page.loadEventFired") return;
  const active = lastActive.get(source.tabId);
  if (!active || Date.now() - active > 10000) return;
  const last = lastCursor.get(source.tabId);
  if (last) moveCursor(source.tabId, last.x, last.y, false);
  else pulseFrame(source.tabId);
});

function forget(tabId) {
  if (tabs.delete(tabId)) send({ method: "Corex.tabClosed", params: { tabId } });
  lastCursor.delete(tabId);
  lastActive.delete(tabId);
}

chrome.debugger.onDetach.addListener((source) => forget(source.tabId));

chrome.tabs.onRemoved.addListener(forget);

chrome.tabs.onCreated.addListener(async (tab) => {
  if (!port || !tabs.has(tab.openerTabId) || tabs.has(tab.id)) return;
  try {
    await control(tab.id);
    send({ method: "Corex.tabOpened", params: { tabId: tab.id, openerTabId: tab.openerTabId } });
  } catch (e) {
    console.warn("Corex: could not follow the new tab:", e);
  }
});

chrome.runtime.onMessage.addListener((req, _sender, sendResponse) => {
  if (req.type === "GET_STATUS") {
    sendResponse({ connected: port !== null, activeTabs: tabs.size });
    return true;
  }
  if (req.type === "FORCE_POLL") {
    connect();
    sendResponse({ ok: true });
    return true;
  }
  if (req.type === "DETACH_ALL") {
    for (const tabId of [...tabs]) {
      forget(tabId);
      chrome.debugger.detach({ tabId }).catch(() => {});
    }
    sendResponse({ ok: true });
    return true;
  }
});

connect();
chrome.alarms.create("corex-connect", { periodInMinutes: 0.5 });
chrome.alarms.onAlarm.addListener(connect);
chrome.runtime.onStartup.addListener(connect);
chrome.runtime.onInstalled.addListener(connect);
