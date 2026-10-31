import fs from "node:fs";
import vm from "node:vm";
import assert from "node:assert/strict";
import path from "node:path";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";

// Executes the actual frontend modules with lightweight hook/Tauri fixtures.
// No DOM renderer, browser UI, native audio, credentials, or network is used.
const repo = path.resolve(process.argv[2] ?? fileURLToPath(new URL("../", import.meta.url)));
const releaseVersion = JSON.parse(fs.readFileSync(path.join(repo, "package.json"), "utf8")).version;
const ts = createRequire(path.join(repo, "package.json"))("typescript");
const deferred = () => { let resolve, reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; };
const clone = value => JSON.parse(JSON.stringify(value));
const provider = { id: "saved-profile", name: "Gemini", kind: "gemini", base_url: "https://example.invalid/v1beta", model: "fixture-model", has_key: true };
const syntheticConfig = { chatgpt_token: "synthetic-token", oai_client_build_number: "keep-build", oai_client_version: "keep-version", oai_device_id: "keep-device", hotkey: "Cmd+Shift+R", recording_format: "wav", audio_device: "Built-in Microphone", app_rules: [], providers: [] };

function fixture(options = {}, entry = "App") {
  const calls = [], listeners = [], emitted = [], opened = [], modules = new Map(), fibers = new Map(), events = new Map(), timers = new Map(), timerDelays = new Map();
  const config = { ...syntheticConfig, ...clone(options.config ?? {}) };
  const history = clone(options.history ?? []);
  let tree, current, dirty = false, recording = false, time = 100000, root, lastHook, uuid = 0, timerId = 0, disposed = false, hookError = "", hookResult;
  const initialHook = {};
  const react = {
    useState(initial) {
      const index = current.cursor++, fiber = current;
      if (!(index in fiber.slots)) fiber.slots[index] = { value: typeof initial === "function" ? initial() : initial };
      return [fiber.slots[index].value, value => { fiber.slots[index].value = typeof value === "function" ? value(fiber.slots[index].value) : value; dirty = true; }];
    },
    useRef(initial) { const index = current.cursor++; if (!(index in current.slots)) current.slots[index] = { current: initial }; return current.slots[index]; },
    useEffect(create, deps) { effect(create, deps, false); },
    useLayoutEffect(create, deps) { effect(create, deps, true); },
    StrictMode: "Fragment",
  };
  function effect(create, deps, layout) {
    const index = current.cursor++, previous = current.slots[index];
    if (!previous || !deps || deps.some((value, i) => !Object.is(value, previous.deps?.[i]))) {
      current.slots[index] = { create, deps, cleanup: previous?.cleanup, pending: true, layout, effect: true };
    }
  }
  const invoke = async (command, args) => {
    calls.push({ command, args });
    if (command === "check_update") { if (options.updateCheckGate) await options.updateCheckGate.promise; if (options.updateCheckFailure) throw new Error("offline"); return options.updateInfo ?? null; }
    if (command === "download_update") { if (options.updateDownloadGate) await options.updateDownloadGate.promise; if (options.updateDownloadFailure) throw new Error("bad signature"); return; }
    if (command === "install_update") { if (options.updateInstallGate) await options.updateInstallGate.promise; if (options.updateInstallFailure) throw new Error("install failed"); return; }
    if (command === "cancel_update") return;
    if (command === "update_system_idle") return options.systemIdle ?? true;
    if (command === "get_config_command") { if (options.configGate) await options.configGate.promise; if (options.configFailure) throw new Error("mock config failure"); return clone(config); }
    if (command === "list_audio_devices") { if (options.devicesGate) await options.devicesGate.promise; return [{ name: "Built-in Microphone", is_default: true }, { name: "USB Microphone", is_default: false }]; }
    if (command === "check_hotkey" || command === "set_voice_status" || command === "show_main_window") return;
    if (command === "get_fn_status" || command === "retry_fn_shortcut") return { enabled: !!config.fn_shortcut_enabled, available: !!config.fn_shortcut_enabled && !!options.fnAvailable, message: options.fnAvailable && config.fn_shortcut_enabled ? "Fn 快捷鍵可使用" : "Fn 權限尚未可用" };
    if (command === "request_input_monitoring") { options.fnAvailable = options.grantOnFnRequest ?? false; return; }
    if (command === "get_insertion_status") { if (options.insertionStatusFailure) throw new Error("mock insertion status failure"); return { trusted: options.trusted ?? false, post_event: options.postEvent ?? options.trusted ?? false }; }
    if (command === "request_post_event_access") {
      if (options.postEventRequestGate) await options.postEventRequestGate.promise;
      if (options.postEventRequestFailure) throw new Error("mock post-event request failure");
      if (options.grantPostOnRequest) options.postEvent = true;
      return options.postEventRequestResult ?? false;
    }
    // Opening system settings does not grant trust; only the simulated user can change it.
    if (command === "request_accessibility" || command === "open_accessibility_settings") { if (options.permissionOpenFailure) throw new Error("mock settings failure"); return; }
    if (command === "reveal_running_app") { if (options.revealFailure) throw new Error("mock reveal failure"); return; }
    if (command === "set_setup_drag_region") { if (options.dragRegionFailure && args.rect) throw new Error("mock drag unavailable"); return; }
    if (command === "get_transcription_history") return clone(history);
    if (command === "start_recording") {
      if (options.startGate) await options.startGate.promise;
      if (options.startFailure) { recording = options.startStatusActive ?? false; throw new Error("mock start failure"); }
      recording = true; return "native-recording-id";
    }
    if (command === "get_recording_status") { if (options.statusFailure) throw new Error("mock status failure"); return { is_recording: recording, recording_id: recording ? "native-recording-id" : null }; }
    if (command === "stop_recording") {
      if (!recording) throw new Error("no active recording");
      if (options.stopGate) await options.stopGate.promise;
      if (options.stopFailure) { if (options.stopEnded) recording = false; throw new Error("mock stop failure"); }
      recording = false; return { audio_path: "/fixture/audio.wav", duration: 2 };
    }
    if (command === "cancel_recording") { if (options.cancelGate) await options.cancelGate.promise; if (options.cancelFailure) throw new Error("mock cancel failure"); recording = false; return; }
    if (command === "complete_dictation") {
      if (options.transcriptionGate) await options.transcriptionGate.promise;
      if (options.transcriptionFailure) throw new Error("mock processing failure");
      time += 3000; return { text: options.empty ? "  " : "測試轉錄", raw_text: "辨識原文", inserted: false, app_name: "TextEdit", mode: args.mode, ...options.result };
    }
    if (command === "test_microphone") { if (options.microphoneGate) await options.microphoneGate.promise; if (options.microphoneFailure) throw new Error("mock no microphone input"); return "收到 2.0 秒音訊，沒有上傳。"; }
    if (command === "test_insertion") {
      if (options.insertionGate) await options.insertionGate.promise;
      if (options.insertionFailure) throw new Error("mock insertion permission denied");
      return "已插入「GPT-Typeless 自動輸入測試」。";
    }
    if (command === "copy_to_clipboard") { if (options.clipboardFailure) throw new Error("mock clipboard failure"); return; }
    if (command === "set_translation_language") {
      if (options.languageSaveGate) await options.languageSaveGate.promise;
      if (options.languageSaveFailure) throw new Error("mock language save failure");
      config.translation_language = args.language; return;
    }
    if (command === "set_config") {
      if (options.saveGate) await options.saveGate.promise;
      if (options.saveFailure) throw new Error("mock save failure");
      Object.assign(config, clone(args.config));
      config.providers = config.providers.map(p => { const next = { ...p, has_key: p.has_key || !!p.api_key }; delete next.api_key; return next; });
      return;
    }
    if (command === "test_provider") return "模擬測試完成";
    if (command === "clear_provider_key") { config.providers = config.providers.map(p => p.id === args.id ? { ...p, has_key: false } : p); return; }
    if (command === "delete_history_item") { const index = history.findIndex(item => item.id === args.id); if (index >= 0) history.splice(index, 1); return; }
    if (command === "clear_all_history") { history.length = 0; return; }
    if (command === "update_history_item") { const item = history.find(item => item.id === args.id); if (item) item.text = args.text; return; }
    throw new Error("Unexpected mocked command: " + command);
  };
  const jsx = (type, props, key) => ({ type, props: props ?? {}, key });
  const window = {
    location: { search: entry === "main-overlay" ? "?overlay=1" : "" },
    innerWidth: 1200, innerHeight: 800,
    addEventListener(name, callback) { if (!events.has(name)) events.set(name, new Set()); events.get(name).add(callback); },
    removeEventListener(name, callback) { events.get(name)?.delete(callback); },
    setInterval(callback, delay) { const id = ++timerId; timers.set(id, callback); timerDelays.set(id, delay); return id; }, clearInterval(id) { timers.delete(id); timerDelays.delete(id); },
    setTimeout(callback, delay) { const id = ++timerId; timers.set(id, callback); timerDelays.set(id, delay); return id; }, clearTimeout(id) { timers.delete(id); timerDelays.delete(id); },
  };
  const document = { activeElement: null, documentElement: { dataset: {}, classList: { toggle() {} } }, getElementById() { return {}; } };
  const context = vm.createContext({ console, window, document, HTMLElement: class {}, URL, URLSearchParams, crypto: { randomUUID: () => `fixture-uuid-${++uuid}` },
    Date: class extends Date { static now() { return time; } },
  });
  const loaded = [];
  function load(relative) {
    const file = path.resolve(repo, "src", relative);
    if (modules.has(file)) return modules.get(file).exports;
    loaded.push(path.relative(repo, file));
    const module = { exports: {} }; modules.set(file, module);
    const source = fs.readFileSync(file, "utf8");
    const compiled = ts.transpileModule(source, { compilerOptions: { target: ts.ScriptTarget.ES2020, module: ts.ModuleKind.CommonJS, jsx: ts.JsxEmit.ReactJSX, esModuleInterop: true } }).outputText;
    const require = name => {
      if (name === "react") return react;
      if (name === "react/jsx-runtime") return { jsx, jsxs: jsx, Fragment: "Fragment" };
      if (name === "react-dom/client") return { createRoot: () => ({ render(value) { root = () => value; dirty = true; } }) };
      if (name === "@tauri-apps/api/core") return { invoke };
      if (name === "@tauri-apps/plugin-opener") return { openUrl: async url => { opened.push(url); if (options.openerFailure) throw new Error("mock opener failure"); } };
      if (name === "@tauri-apps/api/event") return { listen: async (name, callback) => {
        const listener = { name, callback, active: true }; listeners.push(listener);
        if (options.listenerGate) await options.listenerGate.promise;
        return () => { listener.active = false; };
      }, emit: async (name, payload) => { emitted.push({ name, payload }); } };
      if (name === "lucide-react") return new Proxy({}, { get: (_, key) => `Icon:${String(key)}` });
      if (name.endsWith(".css")) return {};
      if (name.startsWith(".")) {
        const target = path.resolve(path.dirname(file), name);
        const resolved = [target, target + ".ts", target + ".tsx"].find(candidate => fs.existsSync(candidate));
        if (resolved) return load(path.relative(path.join(repo, "src"), resolved));
      }
      throw new Error("Unexpected import: " + name);
    };
    vm.runInContext(`(function(require,module,exports){${compiled}\n})`, context, { filename: file })(require, module, module.exports);
    return module.exports;
  }
  function renderComponent(component, props, id) {
    const previous = current;
    let fiber = fibers.get(id);
    if (!fiber) { fiber = { slots: [], cursor: 0 }; fibers.set(id, fiber); }
    fiber.cursor = 0; fiber.seen = true; current = fiber;
    const result = component(props); current = previous; return result;
  }
  function expand(node, id = "0") {
    if (node == null || typeof node === "boolean") return null;
    if (Array.isArray(node)) return node.map((child, i) => expand(child, id + "." + (child?.key ?? i)));
    if (typeof node !== "object") return node;
    if (typeof node.type === "function") {
      const rendered = renderComponent(node.type, node.props, id + ":" + node.type.name);
      return { ...node, rendered: expand(rendered, id + ".r") };
    }
    if (node.props?.className === "setup-drag-source" && node.props.ref) {
      node.props.ref.current = {
        getBoundingClientRect: () => options.dragBounds ?? { left: 180, top: 220, right: 420, bottom: 280 },
        closest: () => ({ getBoundingClientRect: () => options.dialogBounds ?? { left: 100, top: 100, right: 600, bottom: 700 } }),
      };
    }
    return { ...node, props: { ...node.props, children: expand(node.props.children, id + ".c") } };
  }
  function render() {
    dirty = false; for (const fiber of fibers.values()) fiber.seen = false;
    tree = expand(renderComponent(root, {}, "root"));
    for (const [id, fiber] of fibers) if (!fiber.seen) { fiber.slots.forEach(slot => slot.cleanup?.()); fibers.delete(id); }
    for (const layout of [true, false]) for (const fiber of fibers.values()) for (const slot of fiber.slots) if (slot.effect && slot.pending && slot.layout === layout) {
      slot.pending = false; slot.cleanup?.(); slot.cleanup = slot.create();
    }
  }
  function nodes(node) {
    if (node == null || typeof node !== "object") return [];
    if (Array.isArray(node)) return node.flatMap(child => nodes(child));
    return [node, ...nodes(node.props?.children), ...nodes(node.rendered)];
  }
  const text = node => node == null || typeof node === "boolean" ? "" : typeof node !== "object" ? String(node) : Array.isArray(node) ? node.map(text).join("") : text(node.rendered ?? node.props?.children);
  const find = predicate => { const node = nodes(tree).find(predicate); assert.ok(node, "Expected UI element exists"); return node; };
  const byId = id => find(node => node.props?.id === id);
  const byLabel = label => find(node => node.props?.["aria-label"] === label && typeof node.type === "string");
  const button = label => { const matches = nodes(tree).filter(node => node.type === "button" && text(node).trim() === label); assert.ok(matches.length, "Expected button: " + label); return matches.at(-1); };
  const classNode = name => find(node => typeof node.type === "string" && node.props?.className?.split(" ").includes(name));
  const count = command => calls.filter(call => call.command === command).length;
  async function drain() { for (let i = 0; i < 60; i++) { await Promise.resolve(); if (dirty && !disposed && root) render(); } }
  function cleanup() { disposed = true; for (const fiber of fibers.values()) fiber.slots.forEach(slot => slot.cleanup?.()); }
  function strictRestart() { for (const fiber of fibers.values()) fiber.slots.forEach(slot => { if (slot.effect) { slot.cleanup?.(); slot.cleanup = slot.create(); } }); }
  function emit(name, payload) { for (const listener of listeners) if (listener.active && listener.name === name) listener.callback({ payload }); }
  function dispatch(name, payload) { for (const callback of events.get(name) ?? []) callback(payload); }
  if (entry === "hook") {
    const types = load("types.ts");
    Object.assign(initialHook, { config: types.normalizeConfig(config), ready: true, mode: "dictate", onResult: value => { hookResult = value; }, onHistoryRefresh: async () => {}, onError: value => { hookError = value; }, ...options.hook });
    const { useDictation } = load("hooks/useDictation.ts");
    root = () => { lastHook = useDictation(initialHook); return null; };
  } else if (entry.startsWith("main")) load("main.tsx");
  else root = load(entry === "overlay" ? "components/VoiceOverlay.tsx" : "App.tsx").default;
  if (root) render();
  return { options, config, calls, listeners, emitted, opened, loaded, count, drain, cleanup, strictRestart, emit, dispatch, find, byId, byLabel, button, classNode, nodes: node => nodes(node ?? tree), text: () => text(tree), nodeText: text,
    runTimers() { const callbacks = [...timers.values()]; timers.clear(); callbacks.forEach(callback => callback()); },
    runTimerDelay(delay) { for (const [id, callback] of [...timers]) if (timerDelays.get(id) === delay) { timers.delete(id); timerDelays.delete(id); callback(); } },
    hook: () => lastHook, hookError: () => hookError, hookResult: () => hookResult,
    updateHook(patch) { Object.assign(initialHook, patch); dirty = true; },
  };
}
function click(node) { if (!node.props.disabled) return node.props.onClick?.({}); }
const change = (node, value) => node.props.onChange({ target: { value } });
function keyDown(node, options) {
  let prevented = false;
  const event = {
    key: options.key, code: options.code ?? "", repeat: options.repeat ?? false,
    metaKey: options.metaKey ?? false, ctrlKey: options.ctrlKey ?? false,
    altKey: options.altKey ?? false, shiftKey: options.shiftKey ?? false,
    nativeEvent: { isComposing: options.isComposing ?? false },
    getModifierState: name => name === "Fn" ? !!options.fn : name === "AltGraph" ? !!options.altGraph : false,
    preventDefault() { prevented = true; },
  };
  node.props.onKeyDown(event);
  return { event, prevented };
}
const record = screen => click(screen.classNode("record-btn"));
const hotkey = (screen, payload) => screen.emit("hotkey-triggered", payload);
async function settings(screen, tab = "設定") { click(screen.byId("open-settings")); await screen.drain(); click(screen.button(tab)); await screen.drain(); }
async function navigate(screen, name) { click(screen.button(name)); await screen.drain(); }
async function cycle(screen) { record(screen); await screen.drain(); record(screen); await screen.drain(); }

// The default fixture represents an existing user; first-run cases explicitly override the token.
// Inspect the dialog's accessible name or visible title, not its CSS or component structure.
const isDialog = node => node.type === "dialog" || node.props?.role === "dialog";
const setupDialog = screen => screen.nodes().find(node =>
  isDialog(node) &&
  (node.props?.["aria-label"] === "首次設定" || screen.nodeText(node).includes("首次設定"))
);
function findSetupButton(screen, label) {
  // A later step may use a different title; the requested action must still belong to a dialog.
  const matches = screen.nodes().filter(isDialog).flatMap(dialog => screen.nodes(dialog).filter(node => node.type === "button" &&
    (screen.nodeText(node).trim() === label || node.props?.["aria-label"] === label)));
  return matches.at(-1);
}
function setupButton(screen, label) {
  const match = findSetupButton(screen, label);
  assert.ok(match, "Expected setup button: " + label);
  return match;
}
function permissionDialog(screen) {
  const dialog = screen.nodes().find(node => isDialog(node) && screen.nodeText(node).includes("自動輸入") &&
    screen.nodes(node).some(child => child.type === "button" && screen.nodeText(child).trim() === "重新檢查"));
  assert.ok(dialog, "Expected the automatic-input permission step");
  return dialog;
}
function permissionStatus(screen) {
  const status = screen.nodes(permissionDialog(screen)).find(node => node.props?.role === "status");
  assert.ok(status, "Permission status should be exposed to assistive technology");
  return screen.nodeText(status);
}
function assertNoAutomaticPermissions(screen) {
  assert.equal(screen.count("request_accessibility"), 0, "Accessibility must not be requested automatically");
  assert.equal(screen.count("request_post_event_access"), 0, "Posting access must not be requested automatically");
  assert.equal(screen.count("request_input_monitoring"), 0, "Input monitoring must not be requested automatically");
  assert.equal(screen.count("open_accessibility_settings"), 0, "System settings must not open automatically");
  assert.equal(screen.count("reveal_running_app"), 0, "Finder must not open automatically");
}
async function finishPermission(screen) {
  permissionDialog(screen);
  const action = findSetupButton(screen, "開始使用") ?? findSetupButton(screen, "稍後設定");
  assert.ok(action, "The permission step must allow the user to finish or defer");
  click(action); await screen.drain();
  assert.ok(!screen.nodes().some(isDialog), "Finishing the permission step should close setup");
}
async function assertSetupBlocksRecording(screen) {
  assert.equal(screen.classNode("app-shell").props.inert, true, "The app behind setup must be inert");
  assert.equal(screen.classNode("record-btn").props.disabled, true);
  record(screen); hotkey(screen, true); hotkey(screen, false);
  screen.emit("fn-shortcut", "dictate");
  screen.emit("overlay-action", "stop"); screen.emit("overlay-action", "cancel");
  await screen.drain();
  for (const command of ["start_recording", "stop_recording", "cancel_recording", "complete_dictation"]) {
    assert.equal(screen.count(command), 0, command + " must not run while setup is open");
  }
  assert.ok(!screen.calls.some(call => call.command === "set_voice_status" && ["starting", "recording", "processing"].includes(call.args?.status)), "Setup must not activate the recording overlay");
}
async function deferSetup(screen) {
  assert.ok(setupDialog(screen), "Expected 首次設定 dialog");
  click(setupButton(screen, "稍後設定")); await screen.drain();
  assert.ok(!screen.nodes().some(isDialog), "Setup dialog should close");
}

const tests = [
  ["first-run dialog waits for the loaded empty token; an existing token does not open it", async () => {
    for (const token of ["", "synthetic-token"]) {
      const configGate = deferred(), s = fixture({ configGate, config: { chatgpt_token: token } });
      await s.drain();
      assert.ok(!setupDialog(s), "Do not infer first run from the initial default config");
      hotkey(s, true); hotkey(s, false); await s.drain();
      assert.equal(s.count("start_recording"), 0);
      configGate.resolve(); await s.drain();
      assert.equal(!!setupDialog(s), token === "");
      if (!token) {
        assert.ok(s.nodes(setupDialog(s)).some(node => node.props?.id === "setup-token-input"));
        setupButton(s, "稍後設定");
      }
      assert.equal(s.count("set_config"), 0);
    }
  }],
  ["first-run setup can be deferred, closed, and reopened from the homepage", async () => {
    const s = fixture({ config: { chatgpt_token: "" } }); await s.drain();
    await deferSetup(s);
    assert.equal(s.count("set_config"), 0);
    click(s.button("開始設定")); await s.drain();
    const dialog = setupDialog(s); assert.ok(dialog, "Homepage action should reopen setup");
    const close = s.nodes(dialog).find(node => node.type === "button" &&
      /關閉|close/i.test([node.props?.["aria-label"], node.props?.title, s.nodeText(node)].filter(Boolean).join(" ")));
    assert.ok(close, "Expected an accessible close button in setup");
    click(close); await s.drain();
    assert.ok(!s.nodes().some(isDialog));
    assert.equal(s.count("set_config"), 0); assert.equal(s.config.chatgpt_token, "");
    click(s.button("開始設定")); await s.drain(); assert.ok(setupDialog(s));
  }],
  ["record and enabled shortcuts reopen setup instead of recording without a token", async () => {
    const triggers = [
      { name: "record button", run: s => record(s) },
      { name: "hold shortcut", config: { shortcut_behavior: "hold" }, run: s => hotkey(s, true), release: s => hotkey(s, false) },
      { name: "toggle shortcut", config: { shortcut_behavior: "toggle" }, run: s => hotkey(s, true), release: s => hotkey(s, false) },
      { name: "enabled Fn shortcut", config: { fn_shortcut_enabled: true }, run: s => s.emit("fn-shortcut", "dictate") },
    ];
    for (const trigger of triggers) {
      const s = fixture({ config: { chatgpt_token: "", ...trigger.config }, fnAvailable: true }); await s.drain();
      await deferSetup(s);
      trigger.run(s); await s.drain(); trigger.release?.(s); await s.drain();
      assert.equal(s.count("start_recording"), 0, trigger.name);
      assert.ok(setupDialog(s), trigger.name + " should reopen setup");
      assert.equal(s.count("set_config"), 0);
    }
  }],
  ["setup opens only the intended ChatGPT pages without sending pasted data in the URLs", async () => {
    const s = fixture({ config: { chatgpt_token: "" } }); await s.drain();
    change(s.byId("setup-token-input"), "synthetic-secret"); await s.drain();
    click(setupButton(s, "開啟 ChatGPT")); await s.drain();
    click(setupButton(s, "開啟登入資料")); await s.drain();
    assert.deepEqual(s.opened, ["https://chatgpt.com/", "https://chatgpt.com/api/auth/session"]);
    assert.equal(s.count("set_config"), 0);
  }],
  ["onboarding saves bare, Bearer, and session JSON tokens without changing other settings", async () => {
    const inputs = [
      { input: "  synthetic-bare-token \n", token: "synthetic-bare-token" },
      { input: "  bEaReR  synthetic-bearer-token \n", token: "synthetic-bearer-token" },
      { input: '  {"accessToken":"  synthetic-session-token  ","user":{"email":"test@example.invalid"}}  ', token: "synthetic-session-token" },
      { input: '  {"access_token":"synthetic-snake-token","user":{"email":"test@example.invalid"}}  ', token: "synthetic-snake-token" },
      { input: 'Browser viewer\n{\n"user": [preview]\n"accessToken": "synthetic-viewer-token"\n}', token: "synthetic-viewer-token" },
      { input: 'Browser viewer\naccessToken: synthetic-rendered-token\n}', token: "synthetic-rendered-token" },
      { input: 'Browser viewer\naccessToken: synthetic+part/other==\n}', token: "synthetic+part/other==" },
      { input: '{"user":{"accessToken":"synthetic-nested-token"},"accessToken":"synthetic-top-token"}', token: "synthetic-top-token" },
      { input: '"synthetic-quoted-token"', token: "synthetic-quoted-token" },
    ];
    for (const [index, { input, token }] of inputs.entries()) {
      const s = fixture({ config: {
        chatgpt_token: "", audio_device: "USB Microphone", hotkey: "Cmd+Shift+R", shortcut_behavior: "toggle",
        custom_instructions: "Keep dialect", language_variant: "zh-Hant-HK", appearance: "dark",
        app_rules: [{ id: "rule", app_name: "Mail", tone: "formal", instructions: "Keep names" }],
        providers: [provider], active_provider_id: provider.id, future_backend_field: { nested: "preserve" },
      } });
      await s.drain(); assert.ok(setupDialog(s));
      const before = clone(s.config);
      change(s.byId("setup-token-input"), input); await s.drain();
      click(setupButton(s, "儲存並繼續")); await s.drain();
      assert.equal(s.count("set_config"), 1);
      const saved = clone(s.calls.find(call => call.command === "set_config").args.config);
      assert.equal(saved.chatgpt_token, token); assert.equal(s.config.chatgpt_token, token);
      for (const [key, value] of Object.entries(before)) {
        if (key === "chatgpt_token") continue;
        // Normalization adds a blank provider API key; it must not replace the saved native key.
        const sent = key === "providers" ? saved.providers.map(({ api_key, ...rest }) => { assert.equal(api_key, ""); return rest; }) : saved[key];
        assert.deepEqual(sent, value, "sent setting: " + key);
        assert.deepEqual(clone(s.config[key]), value, "persisted setting: " + key);
      }
      assert.ok(!JSON.stringify(saved).includes("test@example.invalid"));
      assert.ok(!JSON.stringify(s.config).includes("test@example.invalid"));
      const microphone = setupButton(s, "測試麥克風");
      setupButton(s, "下一步");
      assert.ok(!findSetupButton(s, "開始使用"), "The microphone step must not finish setup");
      assert.equal(s.count("test_microphone"), 0, "Microphone testing is optional");
      assert.equal(s.count("start_recording"), 0);
      if (index === inputs.length - 1) {
        click(microphone); await s.drain(); assert.equal(s.count("test_microphone"), 1);
      }
      click(setupButton(s, "下一步")); await s.drain();
      assert.match(permissionStatus(s), /尚未|未取得|未授權/);
      assertNoAutomaticPermissions(s);
      await finishPermission(s);
      assert.equal(s.count("set_config"), 1, "Finishing setup must not rewrite the configuration");
      assert.equal(s.config.chatgpt_token, token);
      for (const [key, value] of Object.entries(before)) {
        if (key !== "chatgpt_token") assert.deepEqual(clone(s.config[key]), value, "setting after setup: " + key);
      }
      if (index === 0) {
        assert.equal(s.count("test_microphone"), 0, "Setup can finish without a microphone test");
        record(s); await s.drain(); assert.equal(s.count("start_recording"), 1);
      }
    }
  }],
  ["onboarding save failure keeps the input and token step, and can be retried", async () => {
    const input = '  {"accessToken":"synthetic-session-token","user":{"email":"test@example.invalid"}}  ';
    const options = { saveFailure: true, config: { chatgpt_token: "", audio_device: "USB Microphone" } }, s = fixture(options);
    await s.drain(); const before = clone(s.config);
    change(s.byId("setup-token-input"), input); await s.drain();
    click(setupButton(s, "儲存並繼續")); await s.drain();
    assert.equal(s.count("set_config"), 1); assert.deepEqual(s.config, before);
    assert.ok(setupDialog(s)); assert.equal(s.byId("setup-token-input").props.value, input);
    assert.match(s.nodeText(setupDialog(s)), /儲存失敗/);
    assert.ok(!s.nodes(setupDialog(s)).some(node => node.type === "button" && s.nodeText(node).trim() === "開始使用"));
    assert.equal(s.count("test_microphone"), 0); assert.equal(s.count("start_recording"), 0);
    options.saveFailure = false;
    click(setupButton(s, "儲存並繼續")); await s.drain();
    assert.equal(s.count("set_config"), 2); assert.equal(s.config.chatgpt_token, "synthetic-session-token");
    setupButton(s, "測試麥克風"); setupButton(s, "下一步");
  }],
  ["onboarding rejects a missing session token and API keys without saving or exposing input", async () => {
    for (const input of ['{"user":{"email":"test@example.invalid"}}', '{"user":{"accessToken":"synthetic-nested-token"}}', '{"WARNING_BANNER":"Log in first","sessionToken":"synthetic-other-token"}', '<html>synthetic-login-page</html>', 'ChatGPT start a new chat', 'sessionToken: synthetic-other-token', 'Browser viewer accessToken: "synthetic-unclosed', 'Browser viewer accessToken: synthetic-one accessToken: synthetic-two', 'sk-synthetic-api-key', 'Bearer ']) {
      const s = fixture({ config: { chatgpt_token: "" } }); await s.drain();
      change(s.byId("setup-token-input"), input); await s.drain();
      click(setupButton(s, "儲存並繼續")); await s.drain();
      assert.equal(s.count("set_config"), 0);
      assert.ok(setupDialog(s));
      const alerts = s.nodes(setupDialog(s)).filter(node => node.props?.role === "alert");
      assert.ok(alerts.length);
      assert.ok(!alerts.some(node => s.nodeText(node).includes(input)));
    }
  }],
  ["pasting a multiline browser response extracts only the access token before input sanitization", async () => {
    const s = fixture({ config: { chatgpt_token: "" } }); await s.drain();
    const pasted = 'Pretty print\n{\n"user": [preview]\n"accessToken": "synthetic-clipboard-token"\n"sessionToken": "synthetic-session-secret"\n}';
    let prevented = false;
    s.byId("setup-token-input").props.onPaste({ clipboardData: { getData: () => pasted }, preventDefault: () => { prevented = true; } });
    await s.drain();
    assert.ok(prevented);
    assert.equal(s.byId("setup-token-input").props.value, "synthetic-clipboard-token");
    click(setupButton(s, "儲存並繼續")); await s.drain();
    assert.equal(s.config.chatgpt_token, "synthetic-clipboard-token");
    assert.ok(!JSON.stringify(s.config).includes("synthetic-session-secret"));
  }],
  ["first-run setup progresses from token to microphone to the optional manual permission guide", async () => {
    const s = fixture({ config: { chatgpt_token: "" } }); await s.drain();
    s.byLabel("第 1 步，共 3 步");
    assert.ok(!findSetupButton(s, "開啟權限設定"));
    assertNoAutomaticPermissions(s);
    change(s.byId("setup-token-input"), "synthetic-three-step-token"); await s.drain();
    click(setupButton(s, "儲存並繼續")); await s.drain();
    s.byLabel("第 2 步，共 3 步"); setupButton(s, "測試麥克風");
    assert.ok(!findSetupButton(s, "開啟權限設定"));
    assert.ok(!findSetupButton(s, "開始使用"));
    click(setupButton(s, "下一步")); await s.drain();
    s.byLabel("第 3 步，共 3 步");
    const dialog = permissionDialog(s), guide = s.nodeText(dialog);
    assert.match(guide, /macOS|系統設定/i, "The guide must identify the operating-system setting");
    assert.match(guide, /親自|自行|手動|自己/, "The guide must explain that the user grants permission manually");
    assert.match(guide, /稍後|跳過|可選/, "The guide must explain that permission can be deferred");
    setupButton(s, "開啟權限設定"); setupButton(s, "重新檢查"); setupButton(s, "稍後設定");
    assert.equal(s.count("test_microphone"), 0); assertNoAutomaticPermissions(s);
    click(setupButton(s, "上一步")); await s.drain();
    s.byLabel("第 2 步，共 3 步");
    click(setupButton(s, "下一步")); await s.drain();
    assert.equal(s.config.chatgpt_token, "synthetic-three-step-token");
    assert.equal(s.count("set_config"), 1); assertNoAutomaticPermissions(s);
    await finishPermission(s);
  }],
  ["existing users can open the permission step directly and trust follows manual system changes", async () => {
    const options = { trusted: false, config: {
      auto_insert: true, audio_device: "USB Microphone", custom_instructions: "Keep dialect",
      providers: [provider], active_provider_id: provider.id, future_backend_field: { nested: "preserve" },
    } }, s = fixture(options);
    await s.drain(); const before = clone(s.config);
    assert.ok(!setupDialog(s)); assertNoAutomaticPermissions(s);
    click(s.button("設定權限")); await s.drain();
    permissionDialog(s); s.byLabel("第 3 步，共 3 步");
    assert.ok(!s.nodes().some(node => node.props?.id === "setup-token-input"));
    assert.ok(!findSetupButton(s, "測試麥克風"));
    const untrusted = permissionStatus(s);
    assert.match(untrusted, /尚未|未取得|未授權/);
    setupButton(s, "稍後設定"); assertNoAutomaticPermissions(s);

    click(setupButton(s, "開啟權限設定")); await s.drain();
    assert.equal(s.count("open_accessibility_settings"), 1, "Only an explicit action opens the permission settings");
    assert.equal(options.trusted, false, "Opening settings cannot grant simulated trust");
    assert.equal(permissionStatus(s), untrusted, "Opening settings must not claim permission was granted");

    const checks = s.count("get_insertion_status"), requests = s.count("open_accessibility_settings");
    click(setupButton(s, "重新檢查")); await s.drain();
    assert.ok(s.count("get_insertion_status") > checks, "Recheck must consult the native status");
    assert.equal(s.count("open_accessibility_settings"), requests, "Rechecking must not reopen settings");
    assert.equal(permissionStatus(s), untrusted); assert.equal(options.trusted, false);

    options.trusted = true; const beforeFocus = s.count("get_insertion_status");
    s.dispatch("focus"); await s.drain();
    assert.ok(s.count("get_insertion_status") > beforeFocus, "Returning to the app must refresh trust");
    assert.match(permissionStatus(s), /已取得|已授權|已啟用/);
    setupButton(s, "開始使用");
    options.trusted = false; s.dispatch("focus"); await s.drain();
    assert.match(permissionStatus(s), /尚未|未取得|未授權/, "Revoked permission must not remain trusted");
    setupButton(s, "稍後設定");
    options.trusted = true; s.dispatch("focus"); await s.drain();
    assert.match(permissionStatus(s), /已取得|已授權|已啟用/);
    assert.equal(s.count("open_accessibility_settings"), requests, "Focus checks must not open settings");
    assert.equal(s.count("request_input_monitoring"), 0);
    await finishPermission(s);
    assert.equal(s.count("set_config"), 0); assert.deepEqual(s.config, before);
  }],
  ["an unavailable insertion status stays unknown and can be deferred without requesting permission", async () => {
    const s = fixture({ insertionStatusFailure: true, config: { auto_insert: true } }); await s.drain();
    click(s.button("設定權限")); await s.drain();
    assert.match(permissionStatus(s), /未能確認|尚未確認|無法確認/);
    const checks = s.count("get_insertion_status");
    click(setupButton(s, "重新檢查")); await s.drain();
    assert.ok(s.count("get_insertion_status") > checks);
    assert.match(permissionStatus(s), /未能確認|尚未確認|無法確認/);
    assertNoAutomaticPermissions(s);
    await finishPermission(s);
    assert.equal(s.count("set_config"), 0);
  }],
  ["setup blocks recording shortcuts and overlay actions until the permission guide closes", async () => {
    for (const shortcutBehavior of ["hold", "toggle"]) {
      const s = fixture({ fnAvailable: true, config: { chatgpt_token: "", fn_shortcut_enabled: true, shortcut_behavior: shortcutBehavior } });
      await s.drain(); await assertSetupBlocksRecording(s);
      change(s.byId("setup-token-input"), "synthetic-shortcut-token"); await s.drain();
      click(setupButton(s, "儲存並繼續")); await s.drain(); await assertSetupBlocksRecording(s);
      click(setupButton(s, "下一步")); await s.drain(); permissionDialog(s);
      await assertSetupBlocksRecording(s); assertNoAutomaticPermissions(s);
      await finishPermission(s);
      record(s); await s.drain(); assert.equal(s.count("start_recording"), 1, "Recording should resume after setup closes");

      const existing = fixture({ fnAvailable: true, config: { auto_insert: true, fn_shortcut_enabled: true, shortcut_behavior: shortcutBehavior } });
      await existing.drain(); click(existing.button("設定權限")); await existing.drain();
      permissionDialog(existing); await assertSetupBlocksRecording(existing); assertNoAutomaticPermissions(existing);
      await finishPermission(existing);
      record(existing); await existing.drain(); assert.equal(existing.count("start_recording"), 1);
    }
  }],
  ["permission guide offers to reveal the app in Finder", async () => {
    const s = fixture({ config: { auto_insert: true } }); await s.drain();
    click(s.button("設定權限")); await s.drain();
    const dialog = permissionDialog(s);
    assert.ok(s.nodes(dialog).some(node => ["button", "a"].includes(node.type) &&
      (s.nodeText(node).trim() === "在 Finder 顯示" || node.props?.["aria-label"] === "在 Finder 顯示")), "Expected 在 Finder 顯示 action");
    assertNoAutomaticPermissions(s);
    click(setupButton(s, "在 Finder 顯示")); await s.drain();
    assert.equal(s.count("reveal_running_app"), 1);
    assert.equal(s.count("open_accessibility_settings"), 0);
    assert.equal(s.count("set_config"), 0);
  }],
  ["native drag surface tracks the visible icon and is removed when leaving the step", async () => {
    const options = { config: { auto_insert: true } }, s = fixture(options);
    await s.drain(); click(s.button("設定權限")); await s.drain();
    assert.ok(s.nodes(permissionDialog(s)).some(node => node.props?.className === "setup-drag-source"));
    let regions = s.calls.filter(c => c.command === "set_setup_drag_region").map(c => clone(c.args.rect));
    assert.deepEqual(regions.at(-1), { x: 180, y: 220, width: 240, height: 60, viewportWidth: 1200, viewportHeight: 800 });
    options.dialogBounds = { left: 200, top: 240, right: 350, bottom: 260 };
    s.dispatch("scroll"); await s.drain();
    regions = s.calls.filter(c => c.command === "set_setup_drag_region").map(c => clone(c.args.rect));
    assert.deepEqual(regions.at(-1), { x: 200, y: 240, width: 150, height: 20, viewportWidth: 1200, viewportHeight: 800 });
    click(setupButton(s, "上一步")); await s.drain();
    assert.equal(s.calls.filter(c => c.command === "set_setup_drag_region").at(-1).args.rect, null);
    assertNoAutomaticPermissions(s);
  }],
  ["native drag failures show the Finder fallback without claiming permission", async () => {
    for (const dragRegionFailure of [false, true]) {
      const s = fixture({ dragRegionFailure, config: { auto_insert: true } });
      await s.drain(); click(s.button("設定權限")); await s.drain();
      if (!dragRegionFailure) { s.emit("setup-drag-finished", { outcome: "unavailable" }); await s.drain(); }
      assert.match(s.nodeText(permissionDialog(s)), /未能直接拖曳/);
      assert.match(permissionStatus(s), /尚未取得權限/);
      setupButton(s, "在 Finder 顯示"); assertNoAutomaticPermissions(s);
    }
  }],
  ["a canceled drag offers a retry without claiming a permission change", async () => {
    const s = fixture({ config: { auto_insert: true } });
    await s.drain(); click(s.button("設定權限")); await s.drain();
    s.emit("setup-drag-finished", { outcome: "not-accepted" }); await s.drain();
    assert.match(s.nodeText(permissionDialog(s)), /未完成拖曳/);
    assert.match(permissionStatus(s), /尚未取得權限/);
    setupButton(s, "在 Finder 顯示"); assertNoAutomaticPermissions(s);
    s.emit("setup-drag-finished", { outcome: "accepted" }); await s.drain();
    assert.doesNotMatch(s.nodeText(permissionDialog(s)), /未完成拖曳/);
    assert.match(permissionStatus(s), /尚未取得權限/);
  }],
  ["permission guide reports Finder failures and can retry", async () => {
    const options = { revealFailure: true, config: { auto_insert: true } }, s = fixture(options);
    await s.drain(); click(s.button("設定權限")); await s.drain();
    click(setupButton(s, "在 Finder 顯示")); await s.drain();
    assert.match(s.nodeText(permissionDialog(s)), /未能開啟 Finder/);
    options.revealFailure = false;
    click(setupButton(s, "在 Finder 顯示")); await s.drain();
    assert.doesNotMatch(s.nodeText(permissionDialog(s)), /未能開啟 Finder/);
    assert.equal(s.count("reveal_running_app"), 2);
  }],
  ["permission guide distinguishes Accessibility from permission to post input", async () => {
    const options = { trusted: true, postEvent: false, config: { auto_insert: true } }, s = fixture(options);
    await s.drain(); click(s.button("設定權限")); await s.drain();
    assert.match(permissionStatus(s), /尚未允許自動貼入/);
    setupButton(s, "稍後設定");
    options.postEvent = true; s.dispatch("focus"); await s.drain();
    assert.match(permissionStatus(s), /已取得權限/); setupButton(s, "開始使用");
    assertNoAutomaticPermissions(s);
  }],
  ["post-event request needs a click and never treats its return value as a grant", async () => {
    for (const postEventRequestResult of [false, true]) {
      const options = { trusted: true, postEvent: false, postEventRequestResult, config: { auto_insert: true } }, s = fixture(options);
      await s.drain(); click(s.button("設定權限")); await s.drain();
      assertNoAutomaticPermissions(s);
      const before = s.calls.length;
      click(setupButton(s, "請求自動貼入權限")); await s.drain();
      assert.deepEqual(s.calls.slice(before).map(call => call.command), ["request_post_event_access", "get_insertion_status"]);
      assert.match(permissionStatus(s), /尚未允許自動貼入/);
      assert.doesNotMatch(permissionStatus(s), /已取得權限/);
      setupButton(s, "稍後設定"); setupButton(s, "請求自動貼入權限");
      assert.match(s.nodeText(permissionDialog(s)), /手動複製文字/);
      s.dispatch("focus"); await s.drain();
      assert.equal(s.count("request_post_event_access"), 1, "Focus refresh must not request access");
    }
  }],
  ["post-event request shows success only after refreshed status confirms both permissions", async () => {
    const options = { trusted: true, postEvent: false, grantPostOnRequest: true, config: { auto_insert: true } }, s = fixture(options);
    await s.drain(); click(s.button("設定權限")); await s.drain();
    click(setupButton(s, "請求自動貼入權限")); await s.drain();
    assert.equal(s.count("request_post_event_access"), 1);
    assert.match(permissionStatus(s), /已取得權限/); setupButton(s, "開始使用");
    assert.ok(!findSetupButton(s, "請求自動貼入權限"));
    options.trusted = false; s.dispatch("focus"); await s.drain();
    assert.doesNotMatch(permissionStatus(s), /已取得權限/);
    assert.ok(!findSetupButton(s, "請求自動貼入權限"));
  }],
  ["post-event request failure and unknown refresh remain unconfirmed", async () => {
    for (const failure of ["request", "refresh"]) {
      const options = { trusted: true, postEvent: false, config: { auto_insert: true } }, s = fixture(options);
      await s.drain(); click(s.button("設定權限")); await s.drain();
      if (failure === "request") options.postEventRequestFailure = true;
      else options.insertionStatusFailure = true;
      const before = s.calls.length;
      click(setupButton(s, "請求自動貼入權限")); await s.drain();
      assert.deepEqual(s.calls.slice(before).map(call => call.command), ["request_post_event_access", "get_insertion_status"]);
      assert.doesNotMatch(permissionStatus(s), /已取得權限/);
      assert.match(s.nodeText(permissionDialog(s)), /未能請求自動貼入權限/);
      setupButton(s, "稍後設定");
    }
  }],
  ["post-event request ignores repeated clicks while the request is pending", async () => {
    const gate = deferred(), s = fixture({ trusted: true, postEvent: false, postEventRequestGate: gate, config: { auto_insert: true } });
    await s.drain(); click(s.button("設定權限")); await s.drain();
    const action = setupButton(s, "請求自動貼入權限").props.onClick;
    action(); action(); await s.drain();
    assert.equal(s.count("request_post_event_access"), 1);
    assert.equal(setupButton(s, "請求自動貼入權限").props.disabled, true);
    gate.resolve(); await s.drain();
    assert.equal(setupButton(s, "請求自動貼入權限").props.disabled, false);
    assert.match(permissionStatus(s), /尚未允許自動貼入/);
  }],
  ["mouse click toggles; mouse release and leave never stop recording", async () => {
    const s = fixture(); await s.drain(); record(s); await s.drain();
    s.classNode("record-btn").props.onMouseUp?.({}); s.classNode("record-btn").props.onMouseLeave?.({}); await s.drain();
    assert.equal(s.count("start_recording"), 1); assert.equal(s.count("stop_recording"), 0);
    record(s); await s.drain(); assert.equal(s.count("stop_recording"), 1); assert.equal(s.count("complete_dictation"), 1); assert.equal(s.count("transcribe_audio"), 0);
  }],
  ["startup synchronously rejects stale clicks and keeps the button disabled", async () => {
    const startGate = deferred(), s = fixture({ startGate }); await s.drain(); const stale = s.classNode("record-btn").props.onClick;
    stale(); stale(); await s.drain(); assert.equal(s.count("start_recording"), 1); assert.equal(s.classNode("record-btn").props.disabled, true);
    startGate.resolve(); await s.drain(); assert.equal(s.classNode("record-btn").props.disabled, false); assert.equal(s.count("stop_recording"), 0);
  }],
  ["hold release waits for microphone startup and finishes exactly once", async () => {
    const startGate = deferred(), s = fixture({ startGate }); await s.drain(); hotkey(s, true); hotkey(s, true); hotkey(s, false); hotkey(s, false); await s.drain();
    assert.equal(s.count("stop_recording"), 0); startGate.resolve(); await s.drain(); assert.equal(s.count("start_recording"), 1); assert.equal(s.count("stop_recording"), 1); assert.equal(s.count("complete_dictation"), 1);
  }],
  ["shortcut cannot stop a mouse-owned recording, including during startup", async () => {
    const startGate = deferred(), s = fixture({ startGate }); await s.drain(); record(s); hotkey(s, true); hotkey(s, false); startGate.resolve(); await s.drain(); hotkey(s, true); hotkey(s, false); await s.drain();
    assert.equal(s.count("start_recording"), 1); assert.equal(s.count("stop_recording"), 0);
  }],
  ["a late key release cannot stop a newer mouse session", async () => {
    const s = fixture(); await s.drain(); hotkey(s, true); await s.drain(); record(s); await s.drain(); record(s); await s.drain(); hotkey(s, false); await s.drain(); assert.equal(s.count("start_recording"), 2); assert.equal(s.count("stop_recording"), 1);
  }],
  ["toggle shortcut ignores release and repeat, next physical press finishes", async () => {
    const s = fixture({ config: { shortcut_behavior: "toggle" } }); await s.drain(); hotkey(s, true); hotkey(s, true); hotkey(s, false); await s.drain(); assert.equal(s.count("stop_recording"), 0); hotkey(s, true); hotkey(s, true); await s.drain(); assert.equal(s.count("stop_recording"), 1);
  }],
  ["stopping and processing block stale record/microphone actions and shortcuts", async () => {
    const stopGate = deferred(), transcriptionGate = deferred(), s = fixture({ stopGate, transcriptionGate }, "hook"); await s.drain(); const stale = s.hook(); stale.recordClick(); await s.drain(); stale.recordClick(); await s.drain(); stale.recordClick(); stale.testMicrophone(null); hotkey(s, true); hotkey(s, false); await s.drain(); assert.equal(s.count("start_recording"), 1); assert.equal(s.count("test_microphone"), 0);
    stopGate.resolve(); await s.drain(); stale.recordClick(); stale.testMicrophone(null); await s.drain(); assert.equal(s.count("complete_dictation"), 1); assert.equal(s.count("start_recording"), 1); transcriptionGate.resolve(); await s.drain(); stale.recordClick(); await s.drain(); assert.equal(s.count("start_recording"), 2);
  }],
  ["failed stop recovers ended backend status and permits a new session", async () => {
    const options = { stopFailure: true, stopEnded: true }, s = fixture(options, "hook"); await s.drain(); s.hook().recordClick(); await s.drain(); s.hook().recordClick(); await s.drain(); assert.equal(s.hook().recording, false); assert.equal(s.hook().recordDisabled, false); assert.match(s.hookError(), /mock stop failure/); assert.equal(s.count("complete_dictation"), 0); options.stopFailure = false; s.hook().recordClick(); await s.drain(); assert.equal(s.count("start_recording"), 2);
  }],
  ["failed stop with an active or unknown native status permits stop retry", async () => {
    for (const statusFailure of [false, true]) { const options = { stopFailure: true, statusFailure }, s = fixture(options, "hook"); await s.drain(); s.hook().recordClick(); await s.drain(); s.hook().recordClick(); await s.drain(); assert.equal(s.hook().recording, true); options.stopFailure = false; s.hook().recordClick(); await s.drain(); assert.equal(s.count("start_recording"), 1); assert.equal(s.count("stop_recording"), 2); assert.equal(s.count("complete_dictation"), 1); }
  }],
  ["failed startup releases ownership and permits local microphone testing", async () => {
    const s = fixture({ startFailure: true, statusFailure: true }, "hook"); await s.drain(); s.hook().recordClick(); await s.drain(); assert.match(s.hookError(), /無法開始錄音/); await s.hook().testMicrophone(null); await s.drain(); assert.equal(s.count("test_microphone"), 1);
  }],
  ["an active backend after a startup error remains stoppable", async () => {
    const s = fixture({ startFailure: true, startStatusActive: true }, "hook"); await s.drain(); s.hook().recordClick(); await s.drain(); assert.equal(s.hook().recording, true); s.hook().recordClick(); await s.drain(); assert.equal(s.count("stop_recording"), 1);
  }],
  ["local microphone test uses draft device, keeps draft token, and makes no other command", async () => {
    const s = fixture({ config: { chatgpt_token: "" } }); await s.drain(); await deferSetup(s); await settings(s, "AI 服務"); change(s.byId("token-input"), "synthetic-draft"); await s.drain(); click(s.button("設定")); await s.drain(); change(s.byId("device-select"), "USB Microphone"); await s.drain(); const before = s.calls.length; click(s.byId("test-microphone-btn")); await s.drain(); assert.deepEqual(s.calls.slice(before).map(c => c.command), ["test_microphone"]); assert.equal(s.calls.at(-1).args.deviceName, "USB Microphone"); click(s.button("AI 服務")); await s.drain(); assert.equal(s.byId("token-input").props.value, "synthetic-draft");
  }],
  ["local microphone test locks stale actions and recovers from failure", async () => {
    const microphoneGate = deferred(), options = { microphoneGate, microphoneFailure: true }, s = fixture(options, "hook"); await s.drain(); const stale = s.hook(); stale.testMicrophone(null); stale.testMicrophone(null); stale.recordClick(); hotkey(s, true); hotkey(s, false); await s.drain(); assert.equal(s.count("test_microphone"), 1); assert.equal(s.calls.find(c => c.command === "test_microphone").args.deviceName, undefined); assert.equal(s.count("start_recording"), 0); microphoneGate.resolve(); await s.drain(); assert.equal(s.hook().microphoneError, true); assert.equal(s.hook().testing, false); options.microphoneFailure = false; await s.hook().testMicrophone(null); await s.drain(); assert.equal(s.hook().microphoneError, false);
  }],
  ["microphone test cannot start during recording or delayed startup", async () => {
    const startGate = deferred(), s = fixture({ startGate }, "hook"); await s.drain(); const stale = s.hook(); stale.recordClick(); stale.testMicrophone(null); startGate.resolve(); await s.drain(); stale.testMicrophone(null); await s.drain(); assert.equal(s.count("test_microphone"), 0);
  }],
  ["listeners attach before slow device discovery; config can load independently", async () => {
    const devicesGate = deferred(), s = fixture({ devicesGate }); await s.drain(); assert.ok(s.listeners.some(l => l.name === "hotkey-triggered" && l.active)); hotkey(s, true); await s.drain(); assert.equal(s.count("start_recording"), 1); devicesGate.resolve(); await s.drain();
  }],
  ["shortcuts before config load are ignored, never recording with default credentials", async () => {
    const configGate = deferred(), s = fixture({ configGate }); await s.drain(); hotkey(s, true); hotkey(s, false); await s.drain(); assert.equal(s.count("start_recording"), 0); configGate.resolve(); await s.drain(); hotkey(s, true); await s.drain(); assert.equal(s.count("start_recording"), 1);
  }],
  ["StrictMode retains one listener per event and unmount removes late registrations", async () => {
    const s = fixture(); s.strictRestart(); await s.drain(); assert.equal(s.listeners.filter(l => l.active).length, 3); s.cleanup(); await s.drain(); assert.equal(s.listeners.filter(l => l.active).length, 0);
    const listenerGate = deferred(), late = fixture({ listenerGate }); late.cleanup(); hotkey(late, true); listenerGate.resolve(); await late.drain(); assert.equal(late.listeners.filter(l => l.active).length, 0); assert.equal(late.count("start_recording"), 0);
  }],
  ["native insertion success preserves clipboard and shows inserted output", async () => {
    const s = fixture({ result: { inserted: true } }); await s.drain(); await cycle(s); assert.equal(s.count("copy_to_clipboard"), 0); assert.equal(s.count("show_main_window"), 0); assert.match(s.text(), /已插入文字/); assert.equal(s.classNode("transcription-content").props.children, "測試轉錄"); assert.ok(s.calls.some(c => c.command === "set_voice_status" && c.args.status === "done"));
  }],
  ["unconfirmed posted paste is distinct from no insertion and never encourages a duplicate", async () => {
    const s = fixture({ result: { inserted: false, paste_posted: true, warning: "貼上已送出一次；請先檢查原本輸入位置。", insertion_detail: "Native confirmation timed out." } });
    await s.drain(); await cycle(s);
    assert.match(s.text(), /已送出文字/);
    assert.doesNotMatch(s.text(), /可複製後切換到輸入位置|文字已準備好，可複製貼上|已插入文字/);
    assert.match(s.text(), /原本輸入位置/);
    assert.ok(s.nodes().some(n => n.type === "details" && s.nodeText(n).includes("Native confirmation timed out.")));
    assert.equal(s.count("complete_dictation"), 1); assert.equal(s.count("copy_to_clipboard"), 0);
    assert.equal(s.classNode("transcription-content").props.children, "測試轉錄");
    const hook = fixture({ result: { inserted: false, paste_posted: true } }, "hook"); await hook.drain(); hook.hook().recordClick(); await hook.drain(); hook.hook().recordClick(); await hook.drain();
    assert.equal(hook.hook().message, "已送出文字，請確認原本輸入位置");
  }],
  ["insertion failure exposes a manual copy fallback; copy failure preserves text", async () => {
    const s = fixture({ clipboardFailure: true, result: { warning: "mock insertion unavailable" } }); await s.drain(); await cycle(s); assert.equal(s.count("copy_to_clipboard"), 0); assert.match(s.text(), /mock insertion unavailable/); click(s.classNode("copy-result-btn")); await s.drain(); assert.equal(s.count("copy_to_clipboard"), 1); assert.equal(s.classNode("transcription-content").props.children, "測試轉錄"); assert.match(s.text(), /請選取下方文字/);
  }],
  ["explicit mode and selected device reach start and complete commands", async () => {
    const s = fixture({ config: { audio_device: "USB Microphone" } }); await s.drain(); const mode = s.nodes().find(n => n.type === "button" && s.nodeText(n).includes("編輯選取文字先選取")); click(mode); await s.drain(); await cycle(s); assert.equal(s.calls.find(c => c.command === "start_recording").args.mode, "edit"); assert.equal(s.calls.find(c => c.command === "start_recording").args.deviceName, "USB Microphone"); assert.equal(s.calls.find(c => c.command === "complete_dictation").args.mode, "edit");
  }],
  ["translation bar saves its choice before completing the active recording", async () => {
    const gate = deferred(), s = fixture({ languageSaveGate: gate, config: { translation_language: "English" } }); await s.drain();
    click(s.nodes().find(n => n.type === "button" && s.nodeText(n).includes("翻譯用你習慣"))); await s.drain();
    record(s); await s.drain(); s.emit("overlay-action", { type: "language-select", language: "日本語" }); await s.drain();
    record(s); await s.drain(); assert.equal(s.count("stop_recording"), 0);
    gate.resolve(); await s.drain();
    assert.equal(s.count("set_translation_language"), 1); assert.equal(s.count("set_config"), 0);
    assert.equal(s.config.translation_language, "日本語"); assert.equal(s.count("complete_dictation"), 1);
  }],
  ["a failed translation language save keeps the recording available to finish", async () => {
    const gate = deferred(), options = { languageSaveGate: gate, languageSaveFailure: true }, s = fixture(options); await s.drain();
    click(s.nodes().find(n => n.type === "button" && s.nodeText(n).includes("翻譯用你習慣"))); await s.drain();
    record(s); await s.drain(); s.emit("overlay-action", { type: "language-select", language: "日本語" }); await s.drain(); record(s); await s.drain();
    gate.resolve(); await s.drain(); assert.equal(s.count("stop_recording"), 0); assert.match(s.text(), /錄音仍在繼續/);
    record(s); await s.drain(); assert.equal(s.count("complete_dictation"), 1);
  }],
  ["ask answers appear as answer cards without automatic clipboard copying", async () => {
    const s = fixture(); await s.drain(); click(s.nodes().find(n => n.type === "button" && s.nodeText(n).includes("問 AI用聲音"))); await s.drain(); await cycle(s); assert.ok(s.classNode("answer-card")); assert.equal(s.count("show_main_window"), 1); assert.equal(s.count("copy_to_clipboard"), 0); assert.equal(s.calls.find(c => c.command === "complete_dictation").args.mode, "ask");
  }],
  ["errors and insertion warnings reveal the result without copying it", async () => {
    for (const options of [{ transcriptionFailure: true }, { result: { warning: "保留文字，請檢查" } }]) {
      const s = fixture(options); await s.drain(); await cycle(s);
      assert.equal(s.count("show_main_window"), 1); assert.equal(s.count("copy_to_clipboard"), 0);
    }
  }],
  ["empty or failed dictation never fabricates text or copies output", async () => {
    for (const options of [{ empty: true }, { transcriptionFailure: true }]) { const s = fixture(options, "hook"); await s.drain(); s.hook().recordClick(); await s.drain(); s.hook().recordClick(); await s.drain(); assert.equal(s.hook().status, "error"); assert.equal(s.hookResult(), undefined); assert.equal(s.count("copy_to_clipboard"), 0); assert.equal(s.hook().recordDisabled, false); }
  }],
  ["Escape cancels active recording without transcription or history refresh", async () => {
    const s = fixture(); await s.drain(); record(s); await s.drain(); const before = s.count("get_transcription_history"); s.dispatch("keydown", { key: "Escape", preventDefault() {} }); await s.drain(); assert.equal(s.count("cancel_recording"), 1); assert.equal(s.count("complete_dictation"), 0); assert.equal(s.count("stop_recording"), 0); assert.equal(s.count("get_transcription_history"), before);
  }],
  ["cancellation wins over hold release while microphone startup is pending", async () => {
    const startGate = deferred(), s = fixture({ startGate }, "hook"); await s.drain(); hotkey(s, true); hotkey(s, false); s.emit("overlay-action", "cancel"); startGate.resolve(); await s.drain(); assert.equal(s.count("cancel_recording"), 1); assert.equal(s.count("stop_recording"), 0); assert.equal(s.count("complete_dictation"), 0); assert.equal(s.hook().status, "idle");
  }],
  ["cancel retries after native failure, duplicates share one request", async () => {
    const cancelGate = deferred(), options = { cancelGate, cancelFailure: true }, s = fixture(options, "hook"); await s.drain(); s.hook().recordClick(); await s.drain(); s.hook().cancelRecording(); s.hook().cancelRecording(); await s.drain(); assert.equal(s.count("cancel_recording"), 1); cancelGate.resolve(); await s.drain(); assert.equal(s.hook().recording, true); options.cancelFailure = false; await s.hook().cancelRecording(); await s.drain(); assert.equal(s.hook().status, "idle");
  }],
  ["idle overlay actions do nothing; overlay stop completes only an active session", async () => {
    const s = fixture({}, "hook"); await s.drain(); s.emit("overlay-action", "stop"); s.emit("overlay-action", "cancel"); await s.drain(); assert.equal(s.count("stop_recording"), 0); s.hook().recordClick(); await s.drain(); s.emit("overlay-action", "stop"); s.emit("overlay-action", "stop"); await s.drain(); assert.equal(s.count("stop_recording"), 1);
  }],
  ["configuration retains all existing/new fields and normalizes Bearer token", async () => {
    const rule = { id: "rule", app_name: "Mail", tone: "formal", instructions: "Keep names" };
    const s = fixture({ config: { providers: [provider], active_provider_id: provider.id, app_rules: [rule], custom_instructions: "Keep dialect", language_variant: "zh-Hant-HK", future_backend_field: "preserve" } }); await s.drain(); await settings(s, "AI 服務"); change(s.byId("token-input"), "  bEaReR  synthetic-new  \n"); await s.drain(); click(s.classNode("modal-save-btn")); await s.drain(); const saved = s.calls.find(c => c.command === "set_config").args.config; assert.equal(saved.chatgpt_token, "synthetic-new"); assert.equal(saved.oai_client_version, "keep-version"); assert.equal(saved.future_backend_field, "preserve"); assert.deepEqual(clone(saved.app_rules), [rule]); assert.equal(saved.providers[0].api_key, ""); assert.equal(saved.providers[0].has_key, true); assert.equal(saved.recording_format, "wav"); assert.ok(s.text().includes("v" + releaseVersion));
  }],
  ["blank saved provider key preserves has_key; clearing requires explicit command", async () => {
    const s = fixture({ config: { providers: [provider], active_provider_id: provider.id } }); await s.drain(); await settings(s, "AI 服務"); assert.equal(s.count("clear_provider_key"), 0); assert.equal(s.count("test_provider"), 0); const key = s.nodes().find(n => n.type === "input" && n.props.placeholder === "已儲存，留空即可保留"); assert.equal(key.props.value, ""); click(s.button("立即移除金鑰")); await s.drain(); assert.equal(s.count("clear_provider_key"), 1); assert.equal(s.calls.find(c => c.command === "clear_provider_key").args.id, provider.id); assert.ok(s.nodes().some(n => n.type === "input" && n.props.placeholder === "輸入 API 金鑰"));
  }],
  ["AI profiles save without tests, including Gemini key and selected provider", async () => {
    const s = fixture(); await s.drain(); await settings(s, "AI 服務"); click(s.byId("add-provider")); await s.drain(); const apiKey = s.nodes().find(n => n.type === "input" && n.props.placeholder === "輸入 API 金鑰"); change(apiKey, "synthetic-api-key"); await s.drain(); change(s.byId("active-provider"), "fixture-uuid-1"); await s.drain(); click(s.classNode("modal-save-btn")); await s.drain(); assert.equal(s.count("test_provider"), 0); const saved = s.calls.find(c => c.command === "set_config").args.config; assert.equal(saved.active_provider_id, "fixture-uuid-1"); assert.equal(saved.providers[0].kind, "gemini"); assert.equal(saved.providers[0].api_key, "synthetic-api-key"); await settings(s, "AI 服務"); assert.ok(s.nodes().some(n => n.type === "input" && n.props.placeholder === "已儲存，留空即可保留" && n.props.value === ""));
  }],
  ["AI connection test runs only on the explicit test button", async () => {
    const s = fixture({ config: { providers: [provider] } }); await s.drain(); await settings(s, "AI 服務"); assert.equal(s.count("test_provider"), 0); click(s.button("測試連線（發送請求）")); await s.drain(); assert.equal(s.count("test_provider"), 1); assert.equal(s.calls.find(c => c.command === "test_provider").args.provider.api_key, ""); assert.match(s.text(), /模擬測試完成/);
  }],
  ["saving failure keeps the settings draft and does not change live config", async () => {
    const s = fixture({ saveFailure: true }); await s.drain(); await settings(s, "AI 服務"); change(s.byId("token-input"), "synthetic-draft"); await s.drain(); click(s.classNode("modal-save-btn")); await s.drain(); assert.equal(s.byId("token-input").props.value, "synthetic-draft"); assert.equal(s.config.chatgpt_token, "synthetic-token"); assert.match(s.text(), /mock save failure/);
  }],
  ["shortcut field captures a single key and saves only on confirmation", async () => {
    const s = fixture({ config: { fn_shortcut_enabled: true }, fnAvailable: true }); await s.drain(); await settings(s);
    const field = s.byId("hotkey-input");
    assert.equal(field.props["aria-describedby"], "shortcut-help shortcut-feedback");
    assert.match(s.nodeText(s.byId("shortcut-help")), /單鍵會在其他 App 取代原本功能/);
    assert.match(s.nodeText(s.byId("shortcut-help")), /輸入監控/);
    assert.match(s.nodeText(s.byId("shortcut-help")), /Logitech Options\+/);
    field.props.onFocus(); await s.drain(); assert.match(s.nodeText(s.byId("shortcut-feedback")), /請按下單鍵或組合鍵/);
    assert.equal(keyDown(s.byId("hotkey-input"), { key: "a", code: "KeyA" }).prevented, true); await s.drain();
    assert.equal(s.byId("hotkey-input").props.value, "A");
    assert.equal(s.byId("shortcut-feedback").props.role, "status");
    assert.match(s.nodeText(s.byId("shortcut-feedback")), /儲存後生效/);
    assert.equal(s.count("set_config"), 0); assert.equal(s.config.hotkey, "Cmd+Shift+R");
    click(s.classNode("modal-save-btn")); await s.drain();
    assert.equal(s.config.hotkey, "A"); assert.equal(s.config.fn_shortcut_enabled, true);
    assert.equal(s.count("request_input_monitoring"), 0);
  }],
  ["shortcut capture uses physical keys for function keys, punctuation, and combinations", async () => {
    const s = fixture(); await s.drain(); await settings(s);
    for (const [event, expected] of [
      [{ key: "F1", code: "F1", fn: true }, "F1"],
      [{ key: "F20", code: "F20" }, "F20"],
      [{ key: "R", code: "KeyR", metaKey: true, shiftKey: true }, "Cmd+Shift+R"],
      [{ key: "+", code: "Equal", ctrlKey: true, shiftKey: true }, "Ctrl+Shift+="],
      [{ key: "é", code: "KeyE", altKey: true, altGraph: true }, "Alt+E"],
      [{ key: "Dead", code: "KeyE", altKey: true }, "Alt+E"],
      [{ key: "e", code: "KeyE", ctrlKey: true, altKey: true, altGraph: true }, "Ctrl+Alt+E"],
      [{ key: "+", code: "NumpadAdd" }, "NumpadAdd"],
      [{ key: " ", code: "Space" }, "Space"],
    ]) {
      assert.equal(keyDown(s.byId("hotkey-input"), event).prevented, true); await s.drain();
      assert.equal(s.byId("hotkey-input").props.value, expected);
    }
    click(s.byLabel("重設快捷鍵")); await s.drain();
    assert.equal(s.byId("hotkey-input").props.value, "Cmd+Shift+R");
    assert.match(s.nodeText(s.byId("shortcut-feedback")), /已重設/);
  }],
  ["shortcut capture rejects unsupported keys without changing the draft", async () => {
    const s = fixture(); await s.drain(); await settings(s);
    for (const event of [
      { key: "Shift", code: "ShiftLeft", shiftKey: true }, { key: "Fn", code: "Fn" },
      { key: "CapsLock", code: "CapsLock" }, { key: "F21", code: "F21" },
      { key: "MediaPlayPause", code: "F8" }, { key: "AudioVolumeUp", code: "AudioVolumeUp" },
      { key: "LaunchApplication1", code: "F13" },
      { key: "Unidentified", code: "KeyA" }, { key: "a", code: "KeyA", isComposing: true },
      { key: " ", code: "Space", fn: true },
    ]) {
      assert.equal(keyDown(s.byId("hotkey-input"), event).prevented, true); await s.drain();
      assert.equal(s.byId("hotkey-input").props.value, "Cmd+Shift+R");
      assert.match(s.nodeText(s.byId("shortcut-feedback")), /不能|未能|不支援|Fn 組合鍵/);
    }
    keyDown(s.byId("hotkey-input"), { key: "AltGraph", code: "AltRight" }); await s.drain();
    keyDown(s.byId("hotkey-input"), { key: "a", code: "KeyA", altKey: true, altGraph: true }); await s.drain();
    assert.equal(s.byId("hotkey-input").props.value, "Cmd+Shift+R");
    assert.match(s.nodeText(s.byId("shortcut-feedback")), /不支援 AltGr/);
    s.byId("hotkey-input").props.onKeyUp({ key: "Alt", code: "AltRight" });
    keyDown(s.byId("hotkey-input"), { key: "b", code: "KeyB", repeat: true }); await s.drain();
    assert.equal(s.byId("hotkey-input").props.value, "Cmd+Shift+R");
    assert.equal(s.count("set_config"), 0);
  }],
  ["shortcut navigation and other settings inputs retain normal behavior", async () => {
    const s = fixture(); await s.drain(); await settings(s);
    for (const key of ["Tab", "Escape"]) {
      assert.equal(keyDown(s.byId("hotkey-input"), { key, code: key }).prevented, false);
      assert.equal(s.byId("hotkey-input").props.value, "Cmd+Shift+R");
    }
    const language = s.find(n => n.type === "input" && n.props.list === "settings-languages");
    change(language, "Cantonese"); await s.drain(); assert.equal(s.find(n => n.type === "input" && n.props.list === "settings-languages").props.value, "Cantonese");
    const dialog = s.find(n => n.props?.role === "dialog" && n.props?.["aria-label"] === "設定");
    let stopped = false; dialog.props.onKeyDown({ key: "Escape", stopPropagation() { stopped = true; } }); await s.drain();
    assert.equal(stopped, true); assert.ok(!s.nodes().some(n => n.props?.id === "hotkey-input"));
  }],
  ["shortcut registration failure keeps the draft and old active setting", async () => {
    const s = fixture({ saveFailure: true }); await s.drain(); await settings(s);
    keyDown(s.byId("hotkey-input"), { key: "F13", code: "F13" }); await s.drain();
    click(s.classNode("modal-save-btn")); await s.drain();
    assert.equal(s.byId("hotkey-input").props.value, "F13");
    assert.equal(s.config.hotkey, "Cmd+Shift+R"); assert.match(s.text(), /無法儲存設定.*mock save failure/);
  }],
  ["settings blocks global recording while still allowing local microphone tests", async () => {
    const s = fixture(); await s.drain(); await settings(s); hotkey(s, true); hotkey(s, false); await s.drain(); assert.equal(s.count("start_recording"), 0); click(s.byId("test-microphone-btn")); await s.drain(); assert.equal(s.count("test_microphone"), 1);
  }],
  ["permission request never assumes trust and focus rechecks native status", async () => {
    const options = {}, s = fixture(options); await s.drain(); await settings(s); assert.equal(s.count("request_accessibility"), 0); click(s.button("開啟權限設定")); await s.drain(); assert.equal(s.count("request_accessibility"), 1); assert.match(s.text(), /Device Control and Data Access/); assert.doesNotMatch(s.text(), /已取得權限/); options.trusted = true; s.dispatch("focus"); await s.drain(); assert.match(s.text(), /已取得權限/);
  }],
  ["history uses native IDs for delete and edit; search/filter use real entries", async () => {
    const s = fixture({ history: [{ id: "native-a", text: "香蕉", duration: 2, timestamp: 100000, mode: "dictate" }, { id: "native-b", text: "回答", duration: 3, timestamp: 110000, mode: "ask", app_name: "Safari" }] }); await s.drain(); await navigate(s, "歷史記錄"); change(s.byLabel("搜尋歷史記錄"), "Safari"); await s.drain(); assert.equal(s.nodes().filter(n => n.type === "article").length, 1); click(s.byLabel("編輯記錄")); await s.drain(); const textarea = s.nodes().find(n => n.type === "textarea"); change(textarea, "新回答"); await s.drain(); const form = s.nodes().find(n => n.type === "form"); form.props.onSubmit({ preventDefault() {} }); await s.drain(); assert.equal(s.calls.find(c => c.command === "update_history_item").args.id, "native-b"); click(s.byLabel("刪除記錄")); await s.drain(); assert.equal(s.calls.find(c => c.command === "delete_history_item").args.id, "native-b");
  }],
  ["dictionary edits and snippets persist real config without touching token or provider key", async () => {
    const s = fixture({ config: { providers: [provider], dictionary: [{ id: "word-id", term: "品牌", aliases: ["別名"] }], snippets: [{ id: "snippet-id", trigger: "聯絡", content: "電話文字" }] } }); await s.drain(); await navigate(s, "詞庫"); click(s.byLabel("編輯 品牌")); await s.drain(); const input = s.nodes().find(n => n.type === "input" && n.props.required); change(input, "正確品牌"); await s.drain(); s.nodes().find(n => n.type === "form").props.onSubmit({ preventDefault() {} }); await s.drain(); let saved = s.calls.filter(c => c.command === "set_config").at(-1).args.config; assert.equal(saved.dictionary[0].id, "word-id"); assert.equal(saved.dictionary[0].term, "正確品牌"); assert.equal(saved.chatgpt_token, "synthetic-token"); assert.equal(saved.providers[0].api_key, ""); await navigate(s, "常用短語"); click(s.byLabel("編輯 聯絡")); await s.drain(); change(s.nodes().find(n => n.type === "textarea"), "新的聯絡文字"); await s.drain(); s.nodes().find(n => n.type === "form").props.onSubmit({ preventDefault() {} }); await s.drain(); saved = s.calls.filter(c => c.command === "set_config").at(-1).args.config; assert.equal(saved.snippets[0].id, "snippet-id"); assert.equal(saved.snippets[0].content, "新的聯絡文字");
  }],
  ["overlay entry dynamically excludes App/config initialization and emits only actions", async () => {
    const s = fixture({}, "main-overlay"); await s.drain(); assert.ok(s.loaded.includes("src/components/VoiceOverlay.tsx")); assert.ok(!s.loaded.includes("src/App.tsx")); assert.equal(s.calls.length, 0); assert.deepEqual(s.listeners.map(l => l.name), ["dictation-status", "audio-level"]); s.emit("dictation-status", { status: "recording", mode: "dictate" }); await s.drain(); click(s.byLabel("完成錄音")); click(s.byLabel("取消錄音")); await s.drain(); assert.deepEqual(s.emitted.map(e => [e.name, e.payload]), [["overlay-action", "stop"], ["overlay-action", "cancel"]]); s.emit("dictation-status", { status: "processing" }); await s.drain(); assert.match(s.text(), /處理中/); assert.equal(s.calls.length, 0);
  }],
  ["voice bars follow measured quiet/loud levels and never invoke capture or AI", async () => {
    const s = fixture({}, "overlay"); await s.drain();
    const heights = () => s.nodes().filter(n => n.type === "i").map(n => Number.parseFloat(n.props.style.height));
    s.emit("audio-level", { recording: true, rms: 1 }); await s.drain(); assert.equal(heights().length, 0);
    s.emit("dictation-status", { status: "recording" }); await s.drain(); assert.deepEqual(heights(), Array(13).fill(3));
    s.emit("audio-level", { recording: true, rms: 0 }); await s.drain(); assert.deepEqual(heights(), Array(13).fill(3));
    s.emit("audio-level", { recording: true, rms: 0.04 }); await s.drain(); const quiet = heights().at(-1);
    s.emit("audio-level", { recording: true, rms: 0.5 }); await s.drain(); const loud = heights().at(-1); assert.ok(loud > quiet); assert.ok(quiet > 3);
    s.emit("audio-level", { recording: true, rms: 0.5 }); await s.drain(); assert.equal(heights().at(-1), loud); assert.equal(heights().at(-2), loud);
    s.emit("audio-level", { recording: false, rms: 1 }); await s.drain(); assert.deepEqual(heights(), Array(13).fill(3)); assert.equal(s.calls.length, 0);
  }],
  ["room noise stays near the minimum while actual voice amplitude remains ordered", async () => {
    const s = fixture({}, "overlay"); await s.drain(); s.emit("dictation-status", { status: "recording" }); await s.drain();
    const level = () => Number.parseFloat(s.nodes().filter(n => n.type === "i").at(-1).props.style.height);
    for (const rms of [0, 0.001, 0.009475, 0.013307]) {
      s.emit("audio-level", { recording: true, rms }); await s.drain(); assert.equal(level(), 3);
    }
    const heights = [];
    for (const rms of [0.025, 0.04, 0.06, 0.09]) {
      s.emit("audio-level", { recording: true, rms }); await s.drain(); heights.push(level());
    }
    assert.ok(heights.every((height, i) => i === 0 || height > heights[i - 1]));
    assert.equal(s.count("set_config"), 0); assert.equal(s.count("start_recording"), 0); assert.equal(s.calls.length, 0);
  }],
  ["measured normal speech is visible without raising the accepted silence floor", async () => {
    const s = fixture({}, "overlay"); await s.drain(); s.emit("dictation-status", { status: "recording" }); await s.drain();
    const height = () => Number.parseFloat(s.nodes().filter(n => n.type === "i").at(-1).props.style.height);
    for (const rms of [0.007873, 0.011336, 0.013307]) {
      s.emit("audio-level", { recording: true, rms }); await s.drain(); assert.equal(height(), 3);
    }
    s.emit("audio-level", { recording: true, rms: 0.028359 }); await s.drain(); const normal = height(); assert.ok(normal >= 7 && normal < 15);
    s.emit("audio-level", { recording: true, rms: 0.047977 }); await s.drain(); const stronger = height(); assert.ok(stronger >= 14 && stronger < 23);
    s.emit("audio-level", { recording: true, rms: 0.063131 }); await s.drain(); assert.ok(height() > stronger && height() < 25);
    s.emit("audio-level", { recording: true, rms: 0.146688 }); await s.drain(); assert.equal(height(), 25);
    assert.equal(s.calls.length, 0);
  }],
  ["meter clears on stalled data, invalid payloads and a new recording", async () => {
    const s = fixture({}, "overlay"); await s.drain();
    const heights = () => s.nodes().filter(n => n.type === "i").map(n => Number.parseFloat(n.props.style.height));
    s.emit("dictation-status", { status: "recording" }); await s.drain();
    s.emit("audio-level", { recording: true, rms: 1 }); await s.drain(); assert.equal(heights().at(-1), 25);
    s.runTimers(); await s.drain(); assert.deepEqual(heights(), Array(13).fill(3));
    for (const payload of [null, { recording: true, rms: NaN }, { recording: true, rms: Infinity }, { recording: true, rms: "1" }]) {
      s.emit("audio-level", { recording: true, rms: 1 }); await s.drain(); s.emit("audio-level", payload); await s.drain(); assert.deepEqual(heights(), Array(13).fill(3));
    }
    s.emit("dictation-status", { status: "processing" }); await s.drain(); s.emit("audio-level", { recording: true, rms: 1 }); await s.drain(); assert.equal(heights().length, 0);
    s.emit("dictation-status", { status: "recording" }); await s.drain(); assert.deepEqual(heights(), Array(13).fill(3));
    s.cleanup(); assert.ok(s.listeners.every(l => !l.active));
  }],
  ["waveform CSS has no timed or staggered fake amplitude", async () => {
    const css = fs.readFileSync(path.join(repo, "src/App.css"), "utf8");
    assert.doesNotMatch(css, /@keyframes\s+wave\b|animation:\s*wave\b|\.waveform i:nth-child/);
  }],
  ["overlay cleans up late subscriptions and normal entry does initialize App", async () => {
    const listenerGate = deferred(), overlay = fixture({ listenerGate }, "overlay"); overlay.cleanup(); listenerGate.resolve(); await overlay.drain(); assert.equal(overlay.listeners.filter(l => l.active).length, 0); const main = fixture({}, "main"); await main.drain(); assert.equal(main.count("get_config_command"), 1); assert.ok(!main.loaded.includes("src/components/VoiceOverlay.tsx"));
  }],
  ["Fn defaults off and unavailable Fn never replaces the working Cmd shortcut", async () => {
    const s = fixture({ config: { fn_shortcut_enabled: true }, fnAvailable: false }); await s.drain(); s.emit("fn-shortcut", "translate"); await s.drain(); assert.equal(s.count("start_recording"), 0); assert.ok(!s.nodes().some(n => n.type === "kbd" && s.nodeText(n) === "Fn")); hotkey(s, true); hotkey(s, false); await s.drain(); assert.equal(s.count("complete_dictation"), 1);
    const off = fixture({ fnAvailable: true }); await off.drain(); off.emit("fn-shortcut", "dictate"); await off.drain(); assert.equal(off.count("start_recording"), 0); assert.equal(off.count("request_input_monitoring"), 0);
  }],
  ["Fn single actions start the requested mode and finish the existing mode", async () => {
    const s = fixture({ config: { fn_shortcut_enabled: true }, fnAvailable: true }); await s.drain(); const keys = s.nodes().filter(n => n.type === "kbd").map(s.nodeText); assert.ok(keys.includes("Fn")); assert.ok(keys.includes("Space")); s.emit("fn-shortcut", "translate"); await s.drain(); assert.equal(s.calls.find(c => c.command === "start_recording").args.mode, "translate"); hotkey(s, false); await s.drain(); assert.equal(s.count("stop_recording"), 0); s.emit("fn-shortcut", "ask"); await s.drain(); assert.equal(s.calls.find(c => c.command === "complete_dictation").args.mode, "translate"); s.emit("fn-shortcut", "ask"); await s.drain(); assert.equal(s.calls.filter(c => c.command === "start_recording").at(-1).args.mode, "ask");
  }],
  ["Fn completion waits for startup, can finish mouse ownership, and honors modal locks", async () => {
    const startGate = deferred(), s = fixture({ config: { fn_shortcut_enabled: true }, hook: { fnAvailable: true }, startGate }, "hook"); await s.drain(); s.hook().recordClick(); s.emit("fn-shortcut", "ask"); await s.drain(); assert.equal(s.count("stop_recording"), 0); startGate.resolve(); await s.drain(); assert.equal(s.count("complete_dictation"), 1); assert.equal(s.calls.find(c => c.command === "complete_dictation").args.mode, "dictate"); s.updateHook({ recordingBlocked: true }); await s.drain(); s.emit("fn-shortcut", "ask"); await s.drain(); assert.equal(s.count("start_recording"), 1);
  }],
  ["Fn setting persists without granting permission; explicit settings action rechecks status", async () => {
    const s = fixture(); await s.drain(); await settings(s); click(s.byLabel("使用 Fn 快捷鍵")); await s.drain(); click(s.classNode("modal-save-btn")); await s.drain(); assert.equal(s.calls.find(c => c.command === "set_config").args.config.fn_shortcut_enabled, true); assert.equal(s.count("request_input_monitoring"), 0); await settings(s); click(s.button("開啟輸入監控設定")); await s.drain(); assert.equal(s.count("request_input_monitoring"), 1); assert.match(s.text(), /Fn 權限尚未可用/); assert.ok(!s.nodes().some(n => n.type === "kbd" && s.nodeText(n) === "Fn"));
  }],
  ["setup offers optional Fn shortcuts without automatically requesting system access", async () => {
    const options = { trusted: true }, s = fixture(options); await s.drain();
    const oldHotkey = s.config.hotkey;
    click(s.button("查看權限")); await s.drain();
    assertNoAutomaticPermissions(s);
    assert.match(s.nodeText(permissionDialog(s)), /Fn＋Shift/);
    click(setupButton(s, "啟用 Fn 並設定權限")); await s.drain();
    assert.equal(s.config.fn_shortcut_enabled, true);
    assert.equal(s.config.hotkey, oldHotkey);
    assert.equal(s.count("request_input_monitoring"), 1);
    assert.match(s.nodeText(permissionDialog(s)), /Fn 權限尚未可用/);
    options.fnAvailable = true;
    click(setupButton(s, "重新檢查 Fn")); await s.drain();
    assert.match(s.nodeText(permissionDialog(s)), /Fn 快捷鍵可使用/);
    assert.equal(s.count("request_input_monitoring"), 1, "Rechecking must not request permissions");
    await finishPermission(s);
  }],
  ["Gemini web search defaults off and persists only after an explicit user toggle", async () => {
    const s = fixture({ config: { providers: [provider], active_provider_id: provider.id } }); await s.drain(); click(s.nodes().find(n => n.type === "button" && s.nodeText(n).includes("問 AI用聲音"))); await s.drain(); const toggle = s.byLabel("網上搜尋（Gemini）"); assert.equal(toggle.props["aria-checked"], false); assert.equal(toggle.props.disabled, false); assert.equal(s.count("set_config"), 0); click(toggle); await s.drain(); assert.equal(s.calls.find(c => c.command === "set_config").args.config.web_search_enabled, true); assert.equal(s.byLabel("網上搜尋（Gemini）").props["aria-checked"], true); assert.equal(s.count("test_provider"), 0); assert.equal(s.count("process_text"), 0); assert.equal(s.count("complete_dictation"), 0); assert.equal(s.count("start_recording"), 0); assert.equal(s.opened.length, 0);
  }],
  ["search control explains unavailable providers and cannot opt in without configured Gemini", async () => {
    for (const config of [{}, { providers: [{ ...provider, kind: "openai" }], active_provider_id: provider.id }, { providers: [{ ...provider, has_key: false }], active_provider_id: provider.id }]) {
      const s = fixture({ config }); await s.drain(); await settings(s, "AI 服務"); const toggle = s.byLabel("網上搜尋（Gemini）"); assert.equal(toggle.props.disabled, true); click(toggle); await s.drain(); assert.equal(s.count("set_config"), 0); assert.match(s.text(), /需要選取已設定模型與金鑰的 Gemini/);
    }
  }],
  ["switching away from Gemini clears search and switching back does not re-enable it", async () => {
    const other = { ...provider, id: "other-profile", kind: "openai", name: "OpenAI" }, s = fixture({ config: { providers: [provider, other], active_provider_id: provider.id, web_search_enabled: true } }); await s.drain(); await settings(s, "AI 服務"); change(s.byId("active-provider"), other.id); await s.drain(); assert.equal(s.byLabel("網上搜尋（Gemini）").props["aria-checked"], false); assert.equal(s.byLabel("網上搜尋（Gemini）").props.disabled, true); change(s.byId("active-provider"), provider.id); await s.drain(); assert.equal(s.byLabel("網上搜尋（Gemini）").props["aria-checked"], false); click(s.classNode("modal-save-btn")); await s.drain(); assert.equal(s.calls.find(c => c.command === "set_config").args.config.web_search_enabled, false); assert.equal(s.config.chatgpt_token, "synthetic-token"); assert.equal(s.count("test_provider"), 0);
  }],
  ["changing the active provider kind clears search before saving", async () => {
    const s = fixture({ config: { providers: [provider], active_provider_id: provider.id, web_search_enabled: true } }); await s.drain(); await settings(s, "AI 服務"); const service = s.nodes().find(n => n.type === "select" && n.props.value === "gemini"); change(service, "anthropic"); await s.drain(); assert.equal(s.byLabel("網上搜尋（Gemini）").props["aria-checked"], false); assert.equal(s.byLabel("網上搜尋（Gemini）").props.disabled, true); const model = s.nodes().find(n => n.type === "input" && n.props.placeholder === "輸入供應商提供的模型 ID"); change(model, "fixture-model"); await s.drain(); click(s.classNode("modal-save-btn")); await s.drain(); const saved = s.calls.find(c => c.command === "set_config").args.config; assert.equal(saved.web_search_enabled, false); assert.equal(saved.providers[0].kind, "anthropic"); assert.equal(saved.providers[0].api_key, ""); assert.equal(saved.providers[0].has_key, true);
  }],
  ["canceling search settings discards the opt-in and never calls AI", async () => {
    const s = fixture({ config: { providers: [provider], active_provider_id: provider.id, web_search_enabled: false } }); await s.drain(); await settings(s, "AI 服務"); click(s.byLabel("網上搜尋（Gemini）")); await s.drain(); click(s.button("取消")); await s.drain(); assert.equal(s.config.web_search_enabled, false); assert.equal(s.count("set_config"), 0); assert.equal(s.count("test_provider"), 0); assert.equal(s.count("complete_dictation"), 0);
  }],
  ["grounded answer URLs remain inert until clicked and arbitrary HTML stays text", async () => {
    const text = '<img src=x onerror="alert(1)"> javascript:alert(1)\n來源：https://example.invalid/report?q=1&x=2。\nhttps://name:secret@example.invalid/private\ndata:text/html,<script>alert(1)</script>';
    const s = fixture({ result: { text, mode: "ask" } }); await s.drain(); await cycle(s); const links = s.nodes().filter(n => n.type === "a"); assert.equal(links.length, 1); assert.equal(links[0].props.href, "https://example.invalid/report?q=1&x=2"); assert.ok(!s.nodes().some(n => n.type === "img" || n.type === "script" || n.props?.dangerouslySetInnerHTML)); assert.match(s.nodeText(s.classNode("transcription-content")), /<img src=x/); assert.equal(s.opened.length, 0); let prevented = false; links[0].props.onClick({ preventDefault() { prevented = true; } }); await s.drain(); assert.equal(prevented, true); assert.deepEqual(s.opened, ["https://example.invalid/report?q=1&x=2"]); click(s.classNode("copy-result-btn")); await s.drain(); assert.equal(s.calls.find(c => c.command === "copy_to_clipboard").args.text, text);
  }],
  ["source link open failures keep the answer and show a copyable URL fallback", async () => {
    const text = "來源：https://example.invalid/article", s = fixture({ openerFailure: true, result: { text, mode: "ask" } }); await s.drain(); await cycle(s); const link = s.nodes().find(n => n.type === "a"); link.props.onClick({ preventDefault() {} }); await s.drain(); assert.match(s.text(), /未能開啟來源連結/); assert.equal(s.nodeText(s.classNode("transcription-content")), text); assert.equal(s.count("complete_dictation"), 1);
  }],
  ["manual insertion test invokes only test_insertion and preserves drafts and saved keys on success/failure", async () => {
    for (const insertionFailure of [false, true]) {
      const s = fixture({ insertionFailure, trusted: false, config: { auto_insert: false, providers: [provider], active_provider_id: provider.id } });
      await s.drain(); await settings(s, "AI 服務");
      change(s.byId("token-input"), "synthetic-insertion-draft-token"); await s.drain();
      change(s.nodes().find(n => n.type === "input" && n.props.placeholder === "已儲存，留空即可保留"), "synthetic-insertion-draft-key"); await s.drain();
      click(s.button("設定")); await s.drain(); change(s.byId("device-select"), "USB Microphone"); await s.drain();
      assert.equal(s.count("test_insertion"), 0); assert.equal(s.byId("test-insertion-btn").props.disabled, false);
      assert.match(s.text(), /先在空白測試文件中點選文字插入位置/);
      const savedBefore = clone(s.config), before = s.calls.length;
      click(s.byId("test-insertion-btn")); await s.drain();
      assert.deepEqual(s.calls.slice(before).map(c => c.command), ["test_insertion"]);
      assert.equal(s.calls.at(-1).args, undefined);
      assert.equal(s.byId("insertion-test-result").props.role, insertionFailure ? "alert" : "status");
      assert.match(s.nodeText(s.byId("insertion-test-result")), insertionFailure ? /mock insertion permission denied/ : /已插入「GPT-Typeless 自動輸入測試」/);
      assert.equal(s.byId("device-select").props.value, "USB Microphone");
      click(s.button("AI 服務")); await s.drain();
      assert.equal(s.byId("token-input").props.value, "synthetic-insertion-draft-token");
      assert.equal(s.nodes().find(n => n.type === "input" && n.props.placeholder === "已儲存，留空即可保留").props.value, "synthetic-insertion-draft-key");
      assert.deepEqual(s.config, savedBefore); assert.equal(s.count("set_config"), 0); assert.equal(s.count("request_accessibility"), 0);
    }
  }],
  ["manual insertion test shares the settings work lock and can retry after failure", async () => {
    const insertionGate = deferred(), options = { insertionGate, insertionFailure: true }, s = fixture(options);
    await s.drain(); await settings(s);
    const action = s.byId("test-insertion-btn").props.onClick;
    const before = s.calls.length; action(); action(); await s.drain();
    assert.equal(s.byId("test-insertion-btn").props.disabled, true);
    assert.equal(s.nodeText(s.byId("insertion-test-result")), "正在測試自動輸入…");
    assert.deepEqual(s.calls.slice(before).map(c => c.command), ["test_insertion"]);
    insertionGate.resolve(); await s.drain(); assert.equal(s.byId("insertion-test-result").props.role, "alert");
    options.insertionFailure = false; click(s.byId("test-insertion-btn")); await s.drain();
    assert.equal(s.byId("insertion-test-result").props.role, "status");
    assert.equal(s.byId("test-insertion-btn").props.disabled, false);
    assert.equal(s.count("test_insertion"), 2);
    assert.deepEqual(s.calls.slice(before).map(c => c.command), ["test_insertion", "test_insertion"]);
  }],
  ["automatic update checks by default without blocking a new recording", async () => {
    const updateCheckGate = deferred(), s = fixture({ updateCheckGate, updateInfo: { version: "9.0.0" } });
    await s.drain(); s.runTimerDelay(10_000); await s.drain();
    assert.equal(s.count("check_update"), 1); hotkey(s, true); await s.drain();
    assert.equal(s.count("start_recording"), 1); updateCheckGate.resolve(); await s.drain();
    assert.equal(s.count("download_update"), 1); assert.equal(s.count("install_update"), 0);
  }],
  ["update waits for an uninserted result and the explicit install locks shortcuts", async () => {
    const updateInstallGate = deferred(), s = fixture({ updateInstallGate, updateInfo: { version: "9.0.0" }, result: { text: "unsaved synthetic result", inserted: false } });
    await s.drain(); s.runTimerDelay(10_000); await s.drain(); await cycle(s);
    s.runTimerDelay(300_000); await s.drain(); assert.equal(s.count("install_update"), 0);
    assert.equal(s.button("立即安裝").props.disabled, true); click(s.byLabel("關閉結果")); await s.drain();
    click(s.button("立即安裝")); await s.drain(); assert.equal(s.count("install_update"), 1);
    hotkey(s, true); hotkey(s, false); await s.drain(); assert.equal(s.count("start_recording"), 1);
    updateInstallGate.resolve(); await s.drain();
  }],
  ["manual update remains available with automatic updates disabled", async () => {
    const s = fixture({ config: { auto_update_enabled: false }, updateInfo: { version: "9.0.0" } });
    await s.drain(); s.runTimerDelay(10_000); await s.drain(); assert.equal(s.count("check_update"), 0);
    await settings(s, "關於"); click(s.button("立即檢查")); await s.drain();
    assert.equal(s.count("check_update"), 1); assert.equal(s.count("download_update"), 1);
    click(s.button("取消")); await s.drain(); s.runTimerDelay(300_000); await s.drain(); assert.equal(s.count("install_update"), 0);
    click(s.button("立即安裝")); await s.drain(); assert.equal(s.count("install_update"), 1);
  }],
  ["automatic installation waits for the Mac itself to be idle", async () => {
    const options = { systemIdle: false, updateInfo: { version: "9.0.0" } }, s = fixture(options);
    await s.drain(); s.runTimerDelay(10_000); await s.drain();
    s.runTimerDelay(300_000); await s.drain(); assert.equal(s.count("install_update"), 0);
    options.systemIdle = true; s.runTimerDelay(60_000); await s.drain();
    assert.equal(s.count("install_update"), 1); assert.equal(s.calls.find(c => c.command === "install_update").args.automatic, true);
  }],
];
let failures = 0;
for (const [name, test] of tests) {
  try { await test(); console.log("PASS " + name); }
  catch (error) { failures++; console.log("FAIL " + name + ": " + error.message); if (process.env.DEBUG_TESTS) console.log(error.stack); }
}
console.log(`${tests.length - failures}/${tests.length} mocked frontend scenarios passed.`);
console.log("No browser/native UI, native audio, real credentials, or network calls were used.");
process.exitCode = failures ? 1 : 0;
