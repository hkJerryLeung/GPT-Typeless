import fs from "node:fs";
import vm from "node:vm";
import assert from "node:assert/strict";
import path from "node:path";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";

// Reuse the existing lightweight React/Tauri harness without modifying its tests.
// The fixtures are synthetic; this test never launches the app or reads user data.
const repo = path.resolve(process.argv[2] ?? fileURLToPath(new URL("../", import.meta.url)));
const source = fs.readFileSync(path.join(repo, "test/frontend-regression.mjs"), "utf8");
const begin = source.indexOf("const repo = ");
const end = source.indexOf("// The default fixture represents");
assert.ok(begin >= 0 && end > begin, "frontend fixture is available");
const harness = source.slice(begin, end).replace(/^const repo = .*;$/m, "const repo = fixtureRepo;");
const { fixture, deferred, click } = new Function("fixtureRepo", "fs", "vm", "assert", "path", "createRequire",
  `${harness}\nreturn { fixture, deferred, click };`)(repo, fs, vm, assert, path, createRequire);
const plain = value => JSON.parse(JSON.stringify(value));
const tests = [];
const test = (name, fn) => tests.push([name, fn]);

test("the translation picker uses only language events and leaves focusable controls out", async () => {
  const s = fixture({}, "overlay"); await s.drain();
  s.emit("dictation-status", { status: "recording", mode: "dictate" }); await s.drain();
  assert.ok(!s.nodes().some(node => node.props?.["aria-label"] === "選擇翻譯語言"));
  s.emit("dictation-status", { status: "recording", mode: "translate", translation_language: "English" }); await s.drain();
  let prevented = false;
  s.classNode("overlay-stage").props.onMouseDown({ preventDefault() { prevented = true; } });
  assert.equal(prevented, true);
  click(s.byLabel("選擇翻譯語言")); await s.drain();
  const option = s.nodes().find(node => node.props?.role === "option" && s.nodeText(node) === "日文");
  assert.ok(option); assert.equal(option.props.tabIndex, -1); click(option); await s.drain();
  assert.deepEqual(plain(s.emitted.map(event => [event.name, event.payload])), [
    ["overlay-action", { type: "language-picker", open: true }],
    ["overlay-action", { type: "language-select", language: "日本語" }],
    ["overlay-action", { type: "language-picker", open: false }],
  ]);
  assert.equal(s.calls.length, 0);
  assert.ok(!JSON.stringify(s.emitted).includes("synthetic-token"));
  s.emit("dictation-status", { translation_language: "日本語", language_error: "" }); await s.drain();
  assert.match(s.nodeText(s.byLabel("選擇翻譯語言")), /日文/);
  // A delayed status from an earlier resize must not undo the saved selection.
  s.emit("dictation-status", { status: "recording", mode: "translate", translation_language: "English" }); await s.drain();
  assert.match(s.nodeText(s.byLabel("選擇翻譯語言")), /日文/);
});

test("a custom saved target remains visible while the common targets remain selectable", async () => {
  const s = fixture({}, "overlay"); await s.drain();
  s.emit("dictation-status", { status: "recording", mode: "translate", translation_language: "Italiano" }); await s.drain();
  assert.match(s.nodeText(s.byLabel("選擇翻譯語言")), /Italiano/);
  click(s.byLabel("選擇翻譯語言")); await s.drain();
  assert.ok(s.nodes().some(node => node.props?.role === "option" && s.nodeText(node) === "Italiano" && node.props["aria-selected"]));
});

test("stop waits for the selected language to save before completing this translation", async () => {
  const gate = deferred(); let saved = "English"; const requested = [];
  const s = fixture({ hook: { mode: "translate", onTranslationLanguageChange: async language => {
    requested.push(language); await gate.promise; saved = language;
  } } }, "hook"); await s.drain();
  s.hook().recordClick(); await s.drain();
  s.emit("overlay-action", { type: "language-select", language: "日本語" });
  s.emit("overlay-action", "stop"); await s.drain();
  assert.deepEqual(requested, ["日本語"]); assert.equal(s.count("stop_recording"), 0);
  assert.equal(s.count("complete_dictation"), 0); assert.equal(s.hook().recording, true);
  gate.resolve(); await s.drain();
  assert.equal(saved, "日本語"); assert.equal(s.count("stop_recording"), 1);
  assert.equal(s.count("complete_dictation"), 1);
  assert.equal(s.calls.find(call => call.command === "complete_dictation").args.mode, "translate");
  assert.deepEqual(plain(s.emitted.find(event => event.payload?.translation_language)?.payload), { translation_language: "日本語", language_error: "" });
});

test("a failed save keeps recording and lets the user retry or explicitly stop later", async () => {
  const gate = deferred();
  const s = fixture({ hook: { mode: "translate", onTranslationLanguageChange: async () => gate.promise } }, "hook"); await s.drain();
  s.hook().recordClick(); await s.drain();
  s.emit("overlay-action", { type: "language-select", language: "日本語" });
  s.emit("overlay-action", "stop"); await s.drain();
  gate.reject(new Error("synthetic save failure")); await s.drain();
  assert.equal(s.count("stop_recording"), 0); assert.equal(s.count("complete_dictation"), 0);
  assert.equal(s.hook().recording, true); assert.equal(s.hookError(), "");
  assert.match(s.emitted.find(event => event.payload?.language_error)?.payload.language_error, /未能儲存/);
  s.emit("overlay-action", "stop"); await s.drain();
  assert.equal(s.count("complete_dictation"), 1);
});

test("cancel during a pending save stops the audio without completing the translation", async () => {
  const gate = deferred();
  const s = fixture({ hook: { mode: "translate", onTranslationLanguageChange: async () => gate.promise } }, "hook"); await s.drain();
  s.hook().recordClick(); await s.drain();
  s.emit("overlay-action", { type: "language-select", language: "日本語" });
  s.emit("overlay-action", "stop"); s.emit("overlay-action", "cancel"); await s.drain();
  assert.equal(s.count("cancel_recording"), 1); assert.equal(s.count("complete_dictation"), 0);
  gate.reject(new Error("synthetic save failure")); await s.drain();
  assert.equal(s.hook().status, "idle"); assert.equal(s.count("complete_dictation"), 0);
  assert.match(s.hook().message, /錄音已取消/);
});

test("a save failure during microphone startup still prevents an already requested stop", async () => {
  const startGate = deferred(), saveGate = deferred();
  const s = fixture({ startGate, hook: { mode: "translate", onTranslationLanguageChange: async () => saveGate.promise } }, "hook"); await s.drain();
  s.hook().recordClick();
  s.emit("overlay-action", { type: "language-select", language: "日本語" });
  s.emit("overlay-action", "stop"); await s.drain();
  saveGate.reject(new Error("synthetic save failure")); await s.drain();
  startGate.resolve(); await s.drain();
  assert.equal(s.hook().recording, true); assert.equal(s.count("stop_recording"), 0); assert.equal(s.count("complete_dictation"), 0);
});

test("the main app saves only the selected language and synchronizes its visible setting", async () => {
  const gate = deferred();
  const s = fixture({ languageSaveGate: gate, config: { translation_language: "English" } }); await s.drain();
  const translate = s.nodes().find(node => node.type === "button" && node.props?.className?.includes("mode-row") && s.nodeText(node).includes("翻譯"));
  assert.ok(translate); click(translate); await s.drain();
  click(s.classNode("record-btn")); await s.drain();
  s.emit("overlay-action", { type: "language-select", language: "日本語" });
  s.emit("overlay-action", "stop"); await s.drain();
  assert.equal(s.count("stop_recording"), 0);
  assert.deepEqual(plain(s.calls.find(call => call.command === "set_translation_language").args), { language: "日本語" });
  assert.equal(s.count("set_config"), 0);
  gate.resolve(); await s.drain();
  assert.equal(s.config.translation_language, "日本語");
  assert.equal(s.count("complete_dictation"), 1);
  assert.equal(s.byLabel("翻譯目標語言").props.value, "日本語");
});

test("picker actions outside an active translation cannot persist a language", async () => {
  let saves = 0;
  const s = fixture({ hook: { onTranslationLanguageChange: async () => { saves++; } } }, "hook"); await s.drain();
  s.emit("overlay-action", { type: "language-select", language: "日本語" });
  s.hook().recordClick(); await s.drain();
  s.emit("overlay-action", { type: "language-select", language: "日本語" });
  s.emit("overlay-action", { type: "language-select", language: "unrecognized" }); await s.drain();
  assert.equal(saves, 0);
});

let failed = 0;
for (const [name, run] of tests) {
  try { await run(); console.log(`PASS ${name}`); }
  catch (error) { failed++; console.error(`FAIL ${name}`, error); }
}
if (failed) process.exitCode = 1;
else console.log(`${tests.length}/${tests.length} isolated voice-language scenarios passed.`);
