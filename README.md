<h1 align="center">Notch</h1>

<p align="center">
  <b>A Dynamic Island for Windows that watches your AI agents so you don't have to.</b><br>
  Tiny. Glassy. Hidden until it has something worth saying.
</p>

<p align="center">
  <b>Made for the Windows operating system (Windows 10 and 11).</b> It is a native Windows app, not a cross-platform one.
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

Notch is a **Windows application**: it uses Windows' own media controls, power and device notifications, location service and privacy log, so it runs on Windows 10 and 11 only (not macOS or Linux). It is written in plain Rust on the Win32 API with Direct2D. No Electron, no browser engine, no runtime to install.

## Features

| | |
|---|---|
| **AI agents** | Live progress for **Claude Code**, **Codex**, **Copilot** and any CLI that can run a command. A "working" dot while a run is active, a **Done** or **Needs attention** popup with a chime when it ends, and click-to-focus the window it came from. Each popup says *which agent, which project, and in which app*: `claude · Project Notch · VS Code`. |
| **Agent list** | Hover the notch and the agents page lists **every agent that is working** (for example one in VS Code, one in Orca, one in Chrome) with how long each has been running. An agent that is **waiting for your review gets a green dot** and says *needs review*; the rest keep their orange dot. |
| **Browser AI** | A small Chrome extension reports when **ChatGPT**, **Claude.ai** and **Gemini** start and finish answering, so a background tab can ping you too. |
| **Music** | Now playing from any Windows media app: artwork, animated equalizer, transport controls, a progress bar you can seek, and a volume slider with mute. |
| **System pages** | A widget page first (temperature, the connection with Wi-Fi signal and speed, and the battery level of every connected Bluetooth device), then the system info page: battery (turns green while charging), CPU, GPU, memory, disk and network speed. |
| **Clipboard** | Everything you copy is kept on its own page until you shut down or restart: **search it, filter by date, click to paste anywhere**, and **drag items out or drop files, text and images in**. |
| **Glance strip** | Touch the top edge when nothing is going on: time, battery and connection at a glance, plus **the weather** where you are. |
| **System popups** | Charger plugged or unplugged, low and critical battery, fully charged, Bluetooth device connected (with its battery), network connected, dropped or switched, and a **volume and brightness HUD**. |
| **Privacy dot** | An orange microphone or green camera icon appears whenever any app is using them. |

## Three ways to look at it

```
 hidden          ->   peek (hover the edge)   ->   expanded (hover the notch)
 nothing shown        glance strip, or the         full pages: music, widgets,
 when idle            working dot / now playing    system, agents, clipboard
```

It stays out of the way: with nothing happening it is invisible, and it never takes over a notch you have open.

## Everything it does

### AI agents, in your terminal or editor
- **Works with** Claude Code, Codex, GitHub Copilot, and any command-line tool or script that can run `notch.exe` (see [Connect your agents](#connect-your-agents)). It also runs inside VS Code terminals.
- **Working state:** a dot and a short label appear on the collapsed notch while an agent runs, for example `claude · VS Code`. With several running, the label steps through them every few seconds.
- **Waiting for you:** when an agent stops to ask for your review or input, its dot turns **green** (on the collapsed notch and on the agents page) while the others stay orange. It turns orange again as soon as it works again.
- **Finished or stuck:** when a run ends or needs your input, the notch slides out with a **Done** or **Needs attention** popup and a chime. The popup names the agent, the project and the app, for example `Project Notch · VS Code · Task complete`.
- **Click to jump back:** click the popup and Windows brings the window that agent was running in to the front.
- **Several at once:** each session is tracked on its own, so two Claude Code windows, or Claude and Codex together, never clear each other. With more than one running it shows `N working`.
- **Agent list:** hover the notch to see every running agent on one page, one row each (`claude · Project Notch · VS Code`) with its elapsed time, or *needs review* in green when it is waiting for you. More than five shows `+N more`.
- **Interrupted runs** (Esc, errors) are cleared quietly with no false "Done".

### Music
- Shows what is playing from any app that uses Windows media controls (Spotify, browsers, media players): artwork, title and artist, and an animated equalizer.
- Expanded view has previous / play-pause / next, a **progress bar you can click to seek**, and a **volume slider with mute** (the system volume, same as the keyboard keys).

### System information
- **Widget page (first):** tiles for the temperature, the active connection (Wi-Fi name, signal and speed, or Ethernet) and the battery level of every connected Bluetooth device.
- **System info page (second):** battery (the icon turns green while the charger is connected), CPU, GPU load, memory, disk and network speed.
- Pages stack vertically: scroll with the mouse wheel or click the dots. Values only refresh while you are looking at them.

### Clipboard
The last page keeps a history of what you copy. It lives in memory only: it is gone when you quit the notch, restart or shut down, and nothing is written to disk.
- **Keeps anything:** text, files, folders and images, with their formatting (HTML, RTF) where the app offered it. Up to 200 items, and nothing over 32 MB. Copying the same thing again moves it to the top.
- **Search:** click the search box and type. Every word has to match (case does not matter), for file copies against the file path too. **Enter** pastes the top result, **Esc** clears the search.
- **Date filter:** click the chip to step through *Any date, Today, Yesterday, Last 7 days, Last 30 days*. It combines with the search.
- **Paste anywhere:** click an item and it is pasted into the window you were just using (it sets the clipboard and presses Ctrl+V for you).
- **Drag out:** pull an item off the list onto a folder, an editor, a chat box, anything that accepts a drop. It is always a copy.
- **Drag in:** drag files, text or an image to the top of the screen and the notch opens on this page to take them.
- **Right-click** an item to delete it, or choose *Clear clipboard history*.
- **Private copies are skipped:** password managers mark what they copy as private, and the notch honours that. Copies of virtual files (inside a zip, mail attachments) are not kept either.
- The mouse wheel scrolls the list; at either end it changes page again.

### Glance strip
Touch the top edge when nothing else is going on and the notch shows the **time**, the **battery** and the **connection**, plus the **weather** (a condition symbol and temperature in °C) for where you are.

### System popups
The notch slides out briefly, then hides again, for: charger plugged in or removed, battery low (20%) and critical (10%) with a chime, fully charged, a Bluetooth device connecting (with its battery) or disconnecting, Wi-Fi or Ethernet connecting, dropping or switching, and a **volume and brightness HUD** with a level bar when you use the volume or brightness keys. It never takes over a notch you have open.

### Microphone and camera indicator
An orange microphone or green camera icon appears whenever any app is using them, like the privacy dot on a phone.

### Look and behavior
- **Tinted-glass** design with smooth spring animation, sharp on high-DPI screens.
- **Three modes:** hidden under the camera line, a peek when you touch the edge, and the full view when you hover the notch.
- **Starts at login** and runs in the background. Right-click the notch for the menu (Quit).

## Chrome extension

A small **Manifest V3 Chrome extension** (in the [`extension`](extension) folder) lets the notch follow AI chats in your browser the same way it follows terminal agents.

- **Sites:** ChatGPT (`chatgpt.com`, `chat.openai.com`), **Claude.ai** and **Gemini** (`gemini.google.com`).
- **What you get:** a working dot while the AI is writing its answer, then a **Done** popup with the chat's title and a chime when it finishes. It works while the tab is in the background, so you can switch to other work and get pinged. Two tabs on the same site are tracked separately.
- **How it detects it:** it watches for the site's **Stop** button, which exists only while an answer is being generated. The popup only fires after the button has stayed gone for about 2 seconds, so the short pauses between thinking and writing don't count as finished.
- **Light:** it reacts to page changes (sampled at most every 0.4 s) instead of running a timer, so it adds almost nothing to the page.
- **Private:** the extension never reads your conversation. It sends only the agent name, a status and the tab's title, and only to `127.0.0.1:47615`, the notch running on your own machine. If the notch isn't running, it does nothing.
- **Install:** open `chrome://extensions`, turn on **Developer mode**, click **Load unpacked** and choose the `extension` folder (after `deploy.ps1`, a copy that doesn't depend on the source folder is in `%LOCALAPPDATA%\Notch\extension`). After updating it, press its reload button.
- **Other Chromium browsers** such as Edge can load the same folder, but the popup will still say Chrome.
- **If a site redesigns,** the Stop button selectors in `content.js` are the one thing that may need updating.

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

**Browser AI sites:** install the Chrome extension, see [Chrome extension](#chrome-extension).

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
- The clipboard history is held in memory only and is never written to disk or sent anywhere.
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
  clipboard.rs   clipboard history, search and date filter (pure, unit tested)
  clippage.rs    the clipboard page: drawing, clicks, typing, paste
  dnd.rs         OLE: system clipboard, drag out and drop in
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
