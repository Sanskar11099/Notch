<h1 align="center">Notch</h1>

<p align="center">
  <b>A Dynamic Island for Windows that watches your AI agents so you don't have to.</b><br>
  Tiny. Glassy. Hidden until it has something worth saying.
</p>

<p align="center">
  <img alt="Windows 10/11" src="https://img.shields.io/badge/Windows-10%20%2F%2011-0078D4?logo=windows&logoColor=white">
  <img alt="Rust" src="https://img.shields.io/badge/Rust-Win32%20%2B%20Direct2D-CE422B?logo=rust&logoColor=white">
  <img alt="Executable size" src="https://img.shields.io/badge/exe-~0.4%20MB-2ea44f">
  <img alt="Idle cost" src="https://img.shields.io/badge/idle-0%25%20CPU%20%C2%B7%20~3%20MB%20RAM-2ea44f">
  <img alt="Tests" src="https://img.shields.io/badge/tests-55%20passing-2ea44f">
  <a href="LICENSE"><img alt="MIT License" src="https://img.shields.io/badge/license-MIT-blue"></a>
</p>

---

## What it is

Kick off a long task in Claude Code, Codex or Copilot, switch to another window, and stop checking back. **Notch lives at the top-center of your screen, hides under the camera line when idle, and slides out the moment something happens**: an agent finishes, needs your input, a song changes, you plug in the charger or press a volume key.

Written in plain Rust on the Win32 API with Direct2D. No Electron, no browser engine, no runtime to install.

## Features

| | |
|---|---|
| **AI agents** | Live progress for **Claude Code**, **Codex**, **Copilot** and any CLI that can run a command. A "working" dot while a run is active, a **Done** or **Needs attention** popup with a chime when it ends, and click-to-focus the window it came from. Each popup says *which agent, which project, and in which app*: `claude · Project Notch · VS Code`. |
| **Live step feed** | Hover the notch while an agent works and see its current step, the last few steps, elapsed time and step count. |
| **Browser AI** | A small Chrome extension reports when **ChatGPT**, **Claude.ai** and **Gemini** start and finish answering, so a background tab can ping you too. |
| **Music** | Now playing from any Windows media app: artwork, animated equalizer, transport controls, a progress bar you can seek, and a volume slider with mute. |
| **System page** | Battery (turns green while charging), CPU, memory, disk, network speed, GPU and temperature. A second page shows the connection (Wi-Fi or Ethernet) and the battery level of every connected Bluetooth device. |
| **Glance strip** | Touch the top edge when nothing is going on: time, battery and connection at a glance, plus **the weather** where you are. |
| **System popups** | Charger plugged or unplugged, low and critical battery, fully charged, Bluetooth device connected (with its battery), network connected, dropped or switched, and a **volume and brightness HUD**. |
| **Privacy dot** | An orange microphone or green camera icon appears whenever any app is using them. |

## Three ways to look at it

```
 hidden          ->   peek (hover the edge)   ->   expanded (hover the notch)
 nothing shown        glance strip, or the         full pages: music, system,
 when idle            working dot / now playing    devices, agents
```

It stays out of the way: with nothing happening it is invisible, and it never takes over a notch you have open.

## How it works

```
 Claude Code / Codex / Copilot hooks ─┐
 Chrome extension (ChatGPT, Claude…) ─┼──►  127.0.0.1:47615  ──►  notch.exe  ──►  glass UI
 any script:  notch done "Build ok"  ─┘     (one line per event)      │
                                                                      ├─ media session events (music)
                                                                      ├─ power / device / network / volume notifications
                                                                      └─ registry change watcher (mic / camera)
```

Everything is **event-driven**. There is no polling loop: the notch sleeps until Windows or an agent wakes it, and the animation timer only runs while something is moving.

## Install

You need Windows 10/11 and the [Rust toolchain](https://rustup.rs).

```powershell
git clone https://github.com/Sanskar11099/Notch.git
cd Notch
powershell -File deploy.ps1
```

`deploy.ps1` runs the tests, builds a release binary, installs it to `%LOCALAPPDATA%\Notch`, points start-at-login and the hook wrapper at that copy, and starts it. The installed notch **does not need the source folder**, so you can keep the project on any drive, even a removable one, and re-run `deploy.ps1` whenever you change something.

To just try it without installing: `cargo build --release` and run `target\release\notch.exe`. Right-click the notch to quit.

## Connect your agents

The notch understands a one-line command, so anything that can run a program can report progress:

```powershell
notch.exe working   "Refactoring"  --agent myagent    # show the working dot
notch.exe activity                  --agent myagent    # a step happened (feeds the live view)
notch.exe done      "Task complete" --agent myagent    # popup + chime
notch.exe attention "Needs input"   --agent myagent    # popup + chime
notch.exe clear                     --agent myagent    # drop the dot quietly (interrupted run)
```

If the notch isn't running the command exits in about 80 ms and does nothing, so hooks never slow your agent down.

**Claude Code** (`~/.claude/settings.json`) maps its hook events to those commands:

| Hook | Command |
|---|---|
| `UserPromptSubmit` | `notch.exe working "Working" --agent claude` |
| `PreToolUse` | `notch.exe activity --agent claude` |
| `Stop` | `notch.exe done "Task complete" --agent claude` |
| `Notification` | `notch.exe attention "Needs your input" --agent claude` |
| `StopFailure` | `notch.exe clear --agent claude` |

**Codex** and **Copilot** use their own hook files and call the installed wrapper at `~/.notch/notch.cmd`, which `deploy.ps1` writes for you.

**Browser AI sites:** open `chrome://extensions`, enable *Developer mode*, choose *Load unpacked* and select the `extension` folder (the installed copy is in `%LOCALAPPDATA%\Notch\extension`).

The wire format is a single tab-separated line, sent raw over TCP or as an HTTP POST body:

```
agent <TAB> status <TAB> title [<TAB> hwnd [<TAB> session <TAB> project <TAB> detail]]
```

## Built to stay light

Measured on the author's laptop with everything enabled:

| | |
|---|---|
| Executable | ~0.4 MB |
| Idle CPU | 0 ms per 10 s |
| Idle memory (private working set) | ~1 MB in Task Manager; ~3 MB working set |
| Agent hook when the notch is off | ~80 ms |

How: no polling, partial redraw of only the area that changed, no animation while hidden, a short-lived child process for the weather lookup so its libraries never stay in memory, and the working set trimmed when idle.

## Privacy

- Everything runs on your machine. The event server binds to `127.0.0.1` only.
- The only network request the notch makes is the weather lookup to [Open-Meteo](https://open-meteo.com) (no account, no key), every 30 minutes. It sends your position rounded to about 1 km, taken from the Windows location service. If location is off, the weather simply doesn't show.
- The microphone and camera indicator reads the same Windows privacy log that the taskbar uses. Nothing is recorded or stored.
- No telemetry, no accounts, nothing written to disk except the start-at-login entry.

## Project layout

```
src/
  main.rs        window, rendering, pages, timers, event handling
  server.rs      local event endpoint and the CLI client
  media.rs       Windows media session (music) and seeking
  sysinfo.rs     CPU / memory / disk / GPU / temperature / battery
  netlink.rs     active connection, Wi-Fi signal and speed
  devices.rs     Bluetooth devices and their battery levels
  notice.rs      what each system popup says (pure, unit tested)
  watchers.rs    network and volume change notifications
  weather.rs     location + Open-Meteo, run as a short-lived child
  privacy.rs     microphone / camera in-use watcher
  volume.rs  art.rs  autostart.rs  spring.rs
extension/       Chrome (MV3) extension for ChatGPT, Claude.ai and Gemini
deploy.ps1       test, build and install to %LOCALAPPDATA%\Notch
TESTING.md       manual test checklist for everything above
```

## Development

```powershell
cargo test              # 55 unit tests: parsing, popup logic, labels, weather, and more
cargo build --release   # opt-level z, LTO, panic=abort, stripped
```

[`TESTING.md`](TESTING.md) is a step-by-step manual checklist for the parts that need a real machine: hooks, the extension, charger and Bluetooth popups, and so on.

## Known limits

- The "which app" label is the window that was in front when the agent started working, so switching windows at that exact moment can name the wrong app. Browser agents are always labelled Chrome.
- The volume HUD follows the output device that was the default when the notch started.
- The microphone indicator only shows what Windows logs; some old-style recording APIs are not logged.
- Windows only, and the Chrome extension is loaded unpacked (it is not on the Web Store).

## License

[MIT](LICENSE) © 2026 Sanskar Kumar. Use it, change it, share it.

---

<p align="center"><sub>Built in Rust. Quiet until it matters.</sub></p>
