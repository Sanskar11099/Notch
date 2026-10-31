# Notch test checklist

Run these in order. Each step says what you should see and what to check if it doesn't happen.

## 0. Start the notch (three viewing modes)
1. Run `E:\Project Notch\target\release\notch.exe`. With nothing to show (no agent working, no music playing, no popup) the notch is **hidden** above the screen edge: you see nothing.
2. Push the mouse to the very top edge, in the middle of the screen: the notch slides down into its normal collapsed look (**notch view**).
3. Move the mouse down onto the notch: it opens into the **expanded view** with the pages.
4. Move the mouse away: it collapses and hides again.
5. The notch comes out by itself **only** when a coding agent is working or a completion/attention popup appears, and stays visible until that ends; hovering it then expands it directly. When that ends, it hides again.
6. **Music does not bring the notch out.** While music plays the notch stays hidden; touch the top edge to peek (it then shows the compact art + moving bars) and move down onto it for the music controls.

If you can't see it, it is probably just hidden: touch the top-centre edge. To check it is running, look for a `notch.exe` process in Task Manager (a second launch exits silently if one is already running).

## 1. Claude Code hooks (new session)
Hooks load when a session starts, so open a **new** terminal and run `claude`.

| Do this | Expect |
|---|---|
| Send a prompt, e.g. "list the files here" | An orange dot with "claude" appears in the collapsed pill while it works |
| Wait for the answer | The pill opens with "claude - Done / Task complete" and a chime, then collapses after 3 s |
| Ask for something that needs permission approval | "claude - Needs attention" opens |
| Click the open pill while another window is in front | Your terminal comes to the front |

If nothing shows, run `/hooks` in Claude Code to see whether the notch entries are listed. Direct test:
`%USERPROFILE%/.notch/notch.cmd done "test" --agent claude`

### 1b. Several sessions / project names
Open Claude Code in **two different project folders** (two VS Code or Antigravity windows) and give both a long task.

| Expect | Check |
|---|---|
| Collapsed pill shows "2 working" | With one session it shows "claude · <project folder>" |
| First one finishes: popup says "claude - Done / <project> · Task complete" | The other session's dot stays up (it must not clear) |
| Click the popup | The window that session was started from comes to the front |

The same applies to two Codex sessions, and to two tabs of the same chat site (the extension passes each tab's id).
The project name comes from `CLAUDE_PROJECT_DIR` (Claude Code) or the hook's working folder. If no session id is available the notch falls back to one shared "claude" slot.

## 2. Chrome extension
1. Open `chrome://extensions`, turn on **Developer mode**, click **Load unpacked** and choose `E:\Project Notch\extension`.
2. Reload any open tabs of ChatGPT, Claude.ai and Gemini.
3. On each site, send a prompt that takes a few seconds, such as "Write a 300-word story".

| Site | Expect |
|---|---|
| ChatGPT | Dot "ChatGPT" while streaming, then "ChatGPT - Done" with the chat title |
| Claude.ai | Same, with "Claude.ai" |
| Gemini | Same, with "Gemini" |

If a site doesn't trigger:
- On `chrome://extensions`, check Notch Bridge for errors. Click **service worker** to see its console.
- Check the notch is running first.
- If only one site fails, its Stop button has probably changed. Selectors live in `extension/content.js`.
- Clicking the popup does nothing for browser events. That is expected.

## 3. Codex hooks
1. Start `codex` in any project and send a prompt.
2. Codex may ask you to **trust** the new hooks. If so, approve them. If nothing happens, this is the likeliest cause.

| Do this | Expect |
|---|---|
| Send a prompt | Orange dot "codex" |
| Wait for the answer | "codex - Done / Task complete" and a chime |
| Trigger a permission request | "codex - Needs attention" |

Direct test if it is silent: `%USERPROFILE%/.notch/notch.cmd done "test" --agent codex`

## 3b. Copilot hooks (VS Code)
Hooks live in `~/.copilot/hooks/notch.json` (separate from Orca's `orca.json`; delete the file to undo). In VS Code, open Copilot chat, pick **Copilot** as the Session Target, and send a prompt.

| Do this | Expect |
|---|---|
| Send a prompt | Orange dot "copilot" |
| Wait for the answer | "copilot - Done / Task complete" and a chime |

Already verified with the Copilot CLI (`copilot -p "..."`): the dot appeared and the Done popup opened. The VS Code chat path is still untested.

If nothing happens, the chat may be running as a plain **Local** session, which reads hooks from different places (`chat.useHooks`, `chat.hookFilesLocations`). Note which Session Target you used and tell me. Direct test: `%USERPROFILE%/.notch/notch.cmd done "test" --agent copilot`

Not wired: "needs attention" (Copilot's permission/notification events fire too often to use safely).

## 4. Music
1. Play something in Spotify or a browser tab (e.g. YouTube), then move the mouse away from the notch.
2. The collapsed notch should widen a little: a small album-art square on the left and three bars on the right that bounce up and down (decorative, not tied to the real audio). Pause the music and the bars stop and it returns to the plain narrow pill. Hovering also stops the bars.
3. While an agent is working, you see the orange dot + name on the left and the bars on the right (no art square).
4. Hover over the notch.

| Expect | Check |
|---|---|
| Album art, title, artist | The art may be a gray box for some sources |
| Prev / pause / next buttons | Click pause: the music stops and the icon changes to play |
| Click next / prev | The track changes (only pause has been tested) |
| A progress bar under the artist: elapsed time on the left ("0:54"), total on the right ("1:30"), a thin bar with a small knob | It advances every second while playing and stops while paused. Click the bar to jump to that point (needs a player that allows seeking; browsers and Spotify usually do). Seeking inside the player itself is followed within a second. Sources that report no length (live streams) show no bar |
| Prev / pause / next sit on the left of the bottom row; a speaker icon and a volume slider sit on the right | Click the speaker icon: it mutes (the icon gets a cross and the knob drops to the left); click again to unmute |
| Click or drag on the slider | It sets the **Windows master volume** (same as the keyboard volume keys and the taskbar speaker, not just this app); raising it also unmutes. Dragging past either end clamps at 0% / 100% |
| Press the keyboard volume keys, or change volume anywhere else, while the music page is open | The slider and icon follow within about a second; they are re-read only while the music page is open |

## 4b. System page
1. Hover over the notch. It opens on the music page if a player exists, otherwise straight on the system page.
2. The pages are stacked vertically. Scroll the mouse wheel **down** over the open notch to slide to the next page (system), **up** to go back to music (the content slides up/down with a small spring). Or click one of the two dots in the vertical strip on the right edge.
3. The system page shows Battery (or "Charging" / "Power: Plugged in" on a desktop), CPU, Memory and Disk C: as labelled bars (GPU sits under CPU: it is the busiest GPU engine, the way Task Manager reports it; it shows a dash for the first fraction of a second, or if Windows has no GPU counters; open a game, a video or a WebGL page to see it move), plus a Network row with download (down arrow) and upload (up arrow) speed. Network counts physical, connected adapters only (Wi-Fi/Ethernet; VPN and virtual adapters are not added on top). Start a big download to see it move; it reads 0 for the first second after opening the page.

| Expect | Check |
|---|---|
| Battery % and Disk % match Windows | Battery bar is **green** while the charger is connected (label "Charging", or "Plugged in" when full/not charging); white on battery; orange at 20% or below when unplugged. Unplug and replug the charger to see it change (the colour updates the next second while the page is open). |
| CPU and Memory roughly match Task Manager | These bars turn orange at 85% or above; CPU reads 0 for the first split second |
| Move the mouse away | Sampling stops (nothing is polled while collapsed or on the music page) |
| An agent popup arrives while it's open | The popup takes over; the page returns when it ends if you're still hovering |

## 4c. Devices page (widget tiles: temperature, connection, Bluetooth)
The third page in the vertical stack (scroll down twice from music, or click the third dot). Three rounded widget tiles side by side.

| Expect | Check |
|---|---|
| **Temperature tile**: thermometer icon, a big number like "54°", label | It is the hottest ACPI thermal zone Windows reports without admin rights (a system temperature, not per-core or GPU). Turns orange at 85 °C; shows a dash if the PC exposes no thermal zones |
| **Connection tile**: Wi-Fi (or Ethernet) icon and name, signal bars, the network name, and the link speed | It shows the connection Windows actually routes to the internet over (so a VPN doesn't hide it, and if both Wi-Fi and Ethernet are up it shows the one in use). Bars light by signal quality (Wi-Fi only). Plug in/unplug Ethernet or switch Wi-Fi off to watch it change within ~3 s; "Offline" when there is no route. A weak Wi-Fi signal (under 30%) turns the icon orange |
| **Bluetooth tile**: a round battery ring per connected device, the % inside, the device name underneath | Compare with Settings > Bluetooth & devices. A low (20% or less) battery turns the ring orange. A dash inside the ring means connected but the device doesn't report a battery to Windows |
| More than 3 connected devices | Three rings are shown and the title says "Bluetooth · +N"; none connected says "No device connected" |
| Values refresh every ~3 s while this page is open | Connect or disconnect a device and watch it appear/disappear; nothing is read while collapsed or on other pages |

Classic Bluetooth earbuds and speakers only show a battery if they report one to Windows (LE devices usually do).

## 4d. Agents page (hover preview of running work)
The fourth page. While a Claude Code session is working, **hovering the notch opens straight onto it**.

| Expect | Check |
|---|---|
| Header: orange dot, "claude · <project>", elapsed time and step count | Elapsed time ticks every second while the page is open |
| The task line = the prompt you just sent | Truncated to one line |
| A live feed of the last 5 steps ("Running <command>", "Editing <file>", "Reading <file>", "Searching <pattern>"), oldest at the top | Finished steps have a dim tick; the step it is on right now has a lit orange dot. The feed scrolls up as new steps arrive |
| With several sessions working the page follows the one that stepped most recently | The header adds "+N more" |
| Nothing working: "No agents working" | A run clears when it finishes |

Needs the `PreToolUse` hook in `~/.claude/settings.json` (it calls `notch.exe activity --agent claude`). Start a **new** Claude Code session after it was added. Codex and Copilot show the task text only if their hook payload includes a `prompt`; they do not send live steps.
The task text and commands are shown on screen on hover, so be aware of that if you screen-share.

## 4e. Glance strip (the peek view)
Touch the top-centre edge with nothing playing and no agent working. The collapsed notch shows one line: time (in your own format), a battery icon with its percentage, and the connection icon (Wi-Fi or Ethernet). There is no other number: CPU is on the system page.

| Expect | Check |
|---|---|
| The battery icon is green while the charger is connected, orange when low and unplugged | Matches the battery bar on the system page |
| Values refresh every ~2 s while you keep the cursor on the edge | Nothing is sampled once you move away (the notch hides) |
| It does **not** show while music plays (you get the art + bars instead), an agent works, or a popup is up | |

The strip also shows the current weather at its right end, a condition symbol and the temperature in °C (e.g. "☀ 27°"). It uses the Windows location service (Settings > Privacy & security > Location must allow desktop apps) and Open-Meteo, refreshed every 30 minutes (every 2 minutes until the first success, e.g. no network yet at login). If location is off or offline, the weather simply doesn't appear. Only coordinates rounded to ~1 km are sent. If a refresh fails later, the last reading stays.

## 4f. System popups
The notch slides out for a couple of seconds, then hides again. It never takes over a notch you have open (hovering).

| Event | Popup |
|---|---|
| Plug in / unplug the charger | "Charger connected · Charging · 79%" / "On battery · 78%" |
| Battery drops to 20% or 10% while unplugged | "Battery low" / "Battery critical" with a chime (once each) |
| Battery reaches 100% on the charger | "Fully charged" |
| A Bluetooth device connects / disconnects | The device name with "Connected · 50%" (its battery if it reports one) / "Disconnected" |
| Wi-Fi/Ethernet connects, drops or you switch networks | "Connected" / "Disconnected" / "Switched network" with the name |
| Volume keys (or any volume/mute change from outside the notch) | A volume bar HUD ("Volume · 60%" or "Muted"); it re-arms on every key press |

| Brightness keys (or the brightness slider in Windows) | A "Brightness · 60%" bar HUD with a sun icon, same style as the volume one; the first reading after the notch starts is only a baseline and shows nothing |

Notes: the volume HUD is skipped while you are dragging the notch's own slider. It follows the output device that was the default when the notch started. Bluetooth detection relies on Windows' device-change message and the network one on interface-change notifications; both are debounced (about 1 s) before comparing.

## 4g. Microphone / camera in use
When any app is using the microphone or camera, the notch comes out by itself and shows an orange microphone and/or a green camera icon at the right of the collapsed pill; it hides again when they stop.

| Check | Note |
|---|---|
| Start a call or recording in a browser, Teams, Zoom or Voice Recorder | Windows records usage per app and the notch only reflects what it records. Some old-style recording APIs are not logged by Windows and will not show |
| Compare with the microphone/camera indicator Windows shows in the taskbar tray | They should agree |

## 5. Right-click menu
1. Right-click the notch: you see **Start at login** and **Quit**.
2. Choose **Start at login**, then right-click again: it shows a checkmark.
3. Sign out and back in (or restart): the notch should start by itself. This step has not been tested.
4. If you don't want it, right-click again and uncheck it.
5. Choose **Quit**: the notch disappears.

## What to report back
For each failing step, note:
- the step number
- what you saw instead
- any error text, such as an extension error or a Codex trust prompt

## Cleanup afterward
- Delete the four test conversations created while checking selectors: two in ChatGPT, one in Claude.ai, one in Gemini.
- Orca's existing hooks were left in place and are not affected.

## Known limits
- Interrupting Codex (Esc) quietly clears its "working" dot. Claude Code has no interrupt event: its dot should clear about a minute after Esc, when Claude Code's idle notification arrives (untested). Worst case the dot expires after 30 minutes.
- Browser events cannot focus a window on click.
- Anyone on this machine can send events to the notch (`127.0.0.1:47615`, no authentication).
- The notch is installed outside the project (in `%LOCALAPPDATA%\Notch`) by `deploy.ps1`, which also points `~/.notch/notch.cmd` (used by the Codex and Copilot hooks) and start-at-login at that copy. The Claude Code hooks in `~/.claude/settings.json` call the same exe directly. After moving the project, nothing needs updating: just run `deploy.ps1` from its new location.
