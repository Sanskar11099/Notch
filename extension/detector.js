// Turns "is the AI generating?" samples into working/done events.
// `done` only fires after generation has stayed off for settleMs, so the brief gaps between
// steps of one answer (thinking -> writing, tool use) don't count as finished.
function createDetector({ isGenerating, send, settleMs = 2000, setTimer = setTimeout, clearTimer = clearTimeout }) {
  let generating = false;
  let timer = null;
  return function check() {
    if (isGenerating()) {
      if (timer) {
        clearTimer(timer);
        timer = null;
      }
      if (!generating) {
        generating = true;
        send("working");
      }
    } else if (generating && !timer) {
      timer = setTimer(() => {
        timer = null;
        if (isGenerating()) return;
        generating = false;
        send("done");
      }, settleMs);
    }
  };
}

if (typeof module !== "undefined") module.exports = { createDetector };
