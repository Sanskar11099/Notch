// Relays event lines from content scripts to the notch. Done here (not in the page) because an
// extension service worker with host_permissions is exempt from CORS and local-network blocking.
const ENDPOINT = "http://127.0.0.1:47615/";

chrome.runtime.onMessage.addListener((line, sender) => {
  // Fields 4-5: window handle (unknown from a browser: 0) and the tab id, so two tabs of the same
  // site are tracked as separate runs and one finishing doesn't clear the other.
  const body = `${line}\t0\t${sender.tab ? sender.tab.id : ""}`;
  // Notch not running is a normal state: swallow the connection error.
  fetch(ENDPOINT, { method: "POST", headers: { "Content-Type": "text/plain" }, body }).catch(() => {});
});
