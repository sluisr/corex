async function refresh() {
  const dot = document.getElementById("dot");
  const statusText = document.getElementById("status-text");
  const statusMeta = document.getElementById("status-meta");
  const btnDetach = document.getElementById("btn-detach");

  try {
    const res = await chrome.runtime.sendMessage({ type: "GET_STATUS" });
    if (!res) return;

    if (res.connected) {
      dot.className = "dot connected";
      statusText.textContent = "Ready for Corex";
      statusMeta.innerHTML = `Active tabs: <span>${res.activeTabs}</span>`;
      btnDetach.style.display = res.activeTabs > 0 ? "block" : "none";
    } else {
      dot.className = "dot waiting";
      statusText.textContent = "Not connected";
      statusMeta.innerHTML = "Run <code>cx</code> once in your terminal, then press Reconnect.";
      btnDetach.style.display = "none";
    }
  } catch (e) {
    dot.className = "dot";
    statusText.textContent = "Inactive";
    statusMeta.textContent = "";
  }
}

document.getElementById("btn-reconnect").addEventListener("click", async () => {
  await chrome.runtime.sendMessage({ type: "FORCE_POLL" });
  refresh();
});

document.getElementById("btn-detach").addEventListener("click", async () => {
  await chrome.runtime.sendMessage({ type: "DETACH_ALL" });
  refresh();
});

document.getElementById("version").textContent = "v" + chrome.runtime.getManifest().version;
refresh();
setInterval(refresh, 1500);
