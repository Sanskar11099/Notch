// Run: node extension/detector.test.js
const assert = require("node:assert/strict");
const { createDetector } = require("./detector.js");

function harness() {
  const sent = [];
  let generating = false;
  let pending = null;
  const check = createDetector({
    isGenerating: () => generating,
    send: (s) => sent.push(s),
    setTimer: (fn) => (pending = fn),
    clearTimer: () => (pending = null),
  });
  return {
    sent,
    set: (g) => { generating = g; check(); },
    fireTimer: () => { const fn = pending; pending = null; fn && fn(); },
  };
}

// idle page sends nothing
let h = harness();
h.set(false);
assert.deepEqual(h.sent, []);

// start -> working once (repeat samples don't duplicate); stop -> done only after the settle timer
h = harness();
h.set(true); h.set(true);
assert.deepEqual(h.sent, ["working"]);
h.set(false);
assert.deepEqual(h.sent, ["working"]);
h.fireTimer();
assert.deepEqual(h.sent, ["working", "done"]);

// a gap shorter than the settle time (resumes before timer fires) is not a finish
h = harness();
h.set(true); h.set(false); h.set(true);
h.fireTimer();
assert.deepEqual(h.sent, ["working"]);
h.set(false); h.fireTimer();
assert.deepEqual(h.sent, ["working", "done"]);

// two answers in a row each produce working + done
h.set(true); h.set(false); h.fireTimer();
assert.deepEqual(h.sent, ["working", "done", "working", "done"]);

console.log("detector: all checks passed");
