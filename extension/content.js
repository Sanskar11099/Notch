// A site is "generating" while its Stop button exists. Selectors are best-effort and the most
// likely thing to break when a site redesigns: update `stop` here if a site stops reporting.
const SITES = {
  "chatgpt.com": {
    agent: "ChatGPT",
    // Verified live: aria-label "Stop" (no data-testid). The older selectors are kept as fallbacks.
    stop: ['button[aria-label="Stop"]', 'button[data-testid="stop-button"]', 'button[aria-label="Stop streaming"]'],
  },
  "chat.openai.com": {
    agent: "ChatGPT",
    stop: ['button[data-testid="stop-button"]', 'button[aria-label="Stop streaming"]'],
  },
  "claude.ai": {
    agent: "Claude.ai",
    stop: ['button[aria-label="Stop response"]', 'button[aria-label="Stop Response"]'],
  },
  "gemini.google.com": {
    agent: "Gemini",
    stop: ['button[aria-label="Stop response"]', 'button[aria-label="Stop generating"]'],
  },
};
const CHECK_THROTTLE_MS = 400; // streaming mutates the DOM constantly; sample at most this often
const MAX_TITLE = 80;

(() => {
  const site = SITES[location.hostname];
  if (!site) return;

  const isGenerating = () => site.stop.some((selector) => document.querySelector(selector));
  const send = (status) => {
    const title = (document.title || "").replace(/[\t\r\n]+/g, " ").slice(0, MAX_TITLE);
    try {
      chrome.runtime.sendMessage(`${site.agent}\t${status}\t${title}`);
    } catch {
      // Extension was reloaded; this page's script is orphaned until the tab is refreshed.
    }
  };
  const check = createDetector({ isGenerating, send });

  // Mutation-driven instead of setInterval: hidden tabs get timers throttled, mutations still arrive.
  let queued = false;
  new MutationObserver(() => {
    if (queued) return;
    queued = true;
    setTimeout(() => {
      queued = false;
      check();
    }, CHECK_THROTTLE_MS);
  }).observe(document.documentElement, { childList: true, subtree: true });

  check();
})();
