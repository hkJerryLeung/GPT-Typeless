# GPT-Typeless for macOS

An independent macOS port of [GPT-Typeless](https://github.com/cablate/GPT-Typeless), a voice transcription and AI writing application. This project is not an official Typeless or OpenAI product. It builds on the upstream work by GPTService and contributors; the upstream MIT copyright and licence are included in [LICENSE](LICENSE). The macOS port and its behaviour may differ from upstream.

## Build

The source targets macOS 11 or later. You need Xcode Command Line Tools, Rust and Cargo, and Node.js with npm (Node 20.19+ or 22.12+ is required by Vite 7). Install the appropriate Rust target for your Mac. Dependency installation and the initial build require network access.

```sh
npm ci
npm run test:frontend
node --test test/dictionary-csv.mjs
node test/voice-language.mjs
npm run tauri -- build --bundles app --config '{"bundle":{"createUpdaterArtifacts":false}}'
```

The final command builds a local app bundle without generating signed updater artifacts. It does not install or launch the app. A local build is not automatically Developer ID signed or notarized; macOS may warn about it. Published updates additionally require a suitable Apple signing identity and a Tauri updater signing key matching the configured public key. Those private credentials are not supplied. Keep signing keys outside the project and never commit them. `scripts/release-macos.sh` is for a separately configured release environment.

## Credentials and permissions

1. Open the app's setup or **Settings → AI services**. Speech recognition requires a ChatGPT login access token; an OpenAI API key is a different credential and is not accepted in this field. The setup links to `https://chatgpt.com/api/auth/session` after you sign in. Copy only your own `accessToken` into the app, keep it private like a password, and clear the clipboard afterwards. The token may expire.
2. Optional AI writing, translation and question-answering features require a separate provider profile and the relevant provider's API key and model. Configure these in **Settings → AI services**; sending text to a provider is subject to that provider's policies and charges.
3. Allow microphone access for recording. Accessibility and permission to control/post input to other apps are needed for automatic text insertion. The optional Fn shortcuts additionally need Input Monitoring and Accessibility; they may override the system's normal Fn behaviour. Permissions can be managed in macOS Privacy & Security.

The transcription client sends recorded audio and the ChatGPT bearer token to `https://chatgpt.com/backend-api/transcribe`, an unofficial ChatGPT web backend rather than a supported public transcription API. It may change, stop working, or be subject to service/account restrictions. Use of this backend is at your own discretion; this port is not endorsed by OpenAI.

On macOS, the current code stores the ChatGPT token and provider keys in Keychain. Settings and optional text history are stored locally under `~/Library/Application Support/com.gpttypeless`; recordings are stored in its `recordings` directory. Local history can contain dictated text. The current code attempts to remove recordings older than seven days at startup; recordings can remain until then, and failures can leave files behind. Review local data before sharing a computer or diagnostics.

## Updates and network privacy

Automatic updates are enabled by default in the current source. Once the app is ready it checks a GitHub Releases feed, repeats checks while running, downloads an available update, and can install it after an idle period and restart. Disable **Settings → About → Automatic updates** to stop automatic checks; a manual check still contacts the update service. The configured feed is `https://github.com/hkJerryLeung/GPT-Typeless/releases/latest/download/latest.json` and release downloads may involve GitHub's delivery hosts. They can receive ordinary connection metadata such as your IP address. The updater uses a configured public key to verify signed artifacts; the source does not add the speech token or recordings to its update requests. This describes the code, not an independent network audit or a guarantee about hosting providers. Review the feed and public key before distributing a fork.

Transcription sends audio to ChatGPT, and enabled AI features send their input and related context to the selected provider. Network services, usage policies and retention are controlled by those services. Review the source and settings for your use case.

## Preparing a clean public snapshot

From the private source tree, run the preparation script with an **absolute, new destination outside that tree**. It copies only its explicit file list, creates this document as the snapshot's `README.md`, and refuses symlinked inputs and unsafe destinations. It does not copy Git history, local config, recordings, build outputs, verification docs, project-local certificates or signing keys. It also does not publish anything.

```sh
python3 scripts/prepare-public-snapshot.py /absolute/path/to/new-snapshot
python3 scripts/prepare-public-snapshot.py --check /absolute/path/to/new-snapshot
```

Run `--check` **before installing dependencies or building**: it refuses extra files, including build outputs. Build in a separate disposable copy, or create a fresh snapshot to check again. The snapshot includes both existing `.gitignore` files, with extra certificate and output exclusions added to the snapshot's root copy. Ignore rules do not remove previously tracked files or prevent forced additions.

The check reports paths and finding categories, never matching secret values. It scans the candidate files and, when present, the candidate's own Git index; staged content must match the candidate working files. When the original known index entry is available, its credential is compared in memory against the candidate, without displaying it. If that entry is unavailable, the check reports the limitation. No original Git history is included or cleaned by this process. The rules are heuristic, not a guarantee: review the source, binary assets, attribution, licenses and any staged changes before publishing. New project files are excluded until explicitly added to the script's allowlist.
