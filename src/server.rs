//! Local event endpoint + CLI client. One event per connection, as a raw line or an HTTP POST body:
//!   agent<TAB>status<TAB>title[<TAB>hwnd[<TAB>session<TAB>project[<TAB>detail]]]
//!   status: working | activity | done | attention | clear; hwnd = source window; session tells two runs of the
//!   same agent apart (so one finishing doesn't clear the other); project = folder name shown in the popup;
//!   detail = the task (on `working`) or the current step (on `activity`) for the hover preview.
//! `clear` quietly drops the agent's "working" dot (interrupted/failed turn): no popup, no chime.
//! Binds 127.0.0.1 only. The thread blocks in accept(), so it costs nothing while idle.

use std::{
    io::{Read, Write}, // Read also provides .take()/.read_to_end() on stdin
    net::{TcpListener, TcpStream},
    thread,
    time::Duration,
};
use windows::{
    Win32::{
        Foundation::{CloseHandle, HWND, LPARAM, WPARAM},
        System::Threading::{
            CreateMutexW, OpenMutexW, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
            QueryFullProcessImageNameW, SYNCHRONIZATION_SYNCHRONIZE,
        },
        UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId, PostMessageW, WM_APP},
    },
    core::{PCWSTR, PWSTR, w},
};

// A named mutex the running notch holds. The CLI checks it first: connecting to a closed local port makes
// Windows wait out the whole connect timeout (~0.5 s), which every agent hook would otherwise pay each time
// the notch isn't running.
const RUNNING_FLAG: PCWSTR = w!("Local\\NotchBridgeRunning");

/// Called once by the notch on startup; the handle is kept open until the process exits.
pub fn announce_running() {
    // HANDLE has no destructor, so simply not closing it keeps the mutex alive until the process exits.
    let _ = unsafe { CreateMutexW(None, false, RUNNING_FLAG) };
}

fn notch_is_running() -> bool {
    match unsafe { OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, false, RUNNING_FLAG) } {
        Ok(handle) => {
            let _ = unsafe { CloseHandle(handle) };
            true
        }
        Err(_) => false,
    }
}

pub const PORT: u16 = 47615;
const MAX_REQUEST: usize = 2048;
const MAX_FIELD: usize = 80;
const MAX_DETAIL: usize = 140; // task / current step shown in the hover preview (the UI clips to its width)
const IO_TIMEOUT: Duration = Duration::from_millis(500);
const HTTP_204: &str = "HTTP/1.1 204 No Content\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n";

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Status {
    Working,
    Activity, // a progress update for a run that is already working: no popup, no chime
    Done,
    Attention,
    Clear,
}

#[derive(Debug, Clone)]
pub struct Event {
    pub agent: String,
    pub status: Status,
    pub title: String,
    pub hwnd: isize, // window to focus on click; 0 = unknown
    pub session: String, // empty = unknown: all runs of this agent are treated as one
    pub project: String,
    pub detail: String,
    pub place: String, // the app the agent runs in ("VS Code", "Terminal", "Chrome"); empty = unknown
}

/// Friendly name of the app a window belongs to, empty when unknown. Browser AI events carry no window (hwnd 0),
/// so those are placed in the browser by agent name.
pub fn place_of(hwnd: isize, agent: &str) -> String {
    if hwnd == 0 {
        return if matches!(agent, "ChatGPT" | "Claude.ai" | "Gemini") { "Chrome".to_string() } else { String::new() };
    }
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(HWND(hwnd as *mut _), Some(&mut pid)) };
    let Ok(process) = (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }) else { return String::new() };
    let mut buf = [0u16; 260];
    let mut len = buf.len() as u32;
    let ok = unsafe { QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len) }.is_ok();
    let _ = unsafe { CloseHandle(process) };
    if ok { friendly_app(&String::from_utf16_lossy(&buf[..len as usize])) } else { String::new() }
}

fn friendly_app(exe_path: &str) -> String {
    let file = exe_path.rsplit(['\\', '/']).next().unwrap_or("");
    let stem = file.strip_suffix(".exe").or_else(|| file.strip_suffix(".EXE")).unwrap_or(file);
    match stem.to_lowercase().as_str() {
        "" => String::new(),
        "code" | "code - insiders" => "VS Code".to_string(),
        "windowsterminal" | "cmd" | "powershell" | "pwsh" | "conhost" | "openconsole" => "Terminal".to_string(),
        "chrome" => "Chrome".to_string(),
        "msedge" => "Edge".to_string(),
        other => {
            let mut chars = other.chars();
            chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default()
        }
    }
}

impl Event {
    /// Identity used to track "working" and the click-to-focus window per run, not per agent.
    pub fn key(&self) -> String {
        if self.session.is_empty() { self.agent.clone() } else { format!("{}#{}", self.agent, self.session) }
    }
}

fn clean_n(s: &str, max: usize) -> String {
    s.chars().filter(|c| !c.is_control()).take(max).collect::<String>().trim().to_string()
}

fn clean(s: &str) -> String {
    clean_n(s, MAX_FIELD)
}

pub fn parse(payload: &str) -> Option<Event> {
    let mut fields = payload.trim().splitn(7, '\t');
    let agent = clean(fields.next()?);
    let status = match fields.next()?.trim() {
        "working" => Status::Working,
        "activity" => Status::Activity,
        "done" => Status::Done,
        "attention" => Status::Attention,
        "clear" => Status::Clear,
        _ => return None,
    };
    let title = clean(fields.next().unwrap_or(""));
    let hwnd = fields.next().and_then(|f| f.trim().parse().ok()).unwrap_or(0);
    let session = clean(fields.next().unwrap_or(""));
    let project = clean(fields.next().unwrap_or(""));
    let detail = clean_n(fields.next().unwrap_or(""), MAX_DETAIL);
    let place = place_of(hwnd, &agent);
    (!agent.is_empty()).then_some(Event { agent, status, title, hwnd, session, project, detail, place })
}

fn payload_of(raw: &str) -> &str {
    if raw.starts_with("POST ") { raw.split_once("\r\n\r\n").map_or("", |(_, body)| body) } else { raw }
}

fn read_request(stream: &mut TcpStream) -> String {
    let mut buf = Vec::with_capacity(256);
    let mut chunk = [0u8; 512];
    while buf.len() < MAX_REQUEST {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
        let text = String::from_utf8_lossy(&buf);
        // Raw clients end with a newline; HTTP clients are done once a body follows the headers.
        let done = if text.starts_with("POST ") { !payload_of(&text).is_empty() } else { text.contains('\n') };
        if done {
            break;
        }
    }
    String::from_utf8_lossy(&buf).into_owned()
}

fn handle(mut stream: TcpStream) -> Option<Event> {
    let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
    let raw = read_request(&mut stream);
    if raw.starts_with("POST ") || raw.starts_with("OPTIONS ") {
        let _ = stream.write_all(HTTP_204.as_bytes());
    }
    parse(payload_of(&raw))
}

/// `None` means another notch already owns the port.
pub fn bind() -> Option<TcpListener> {
    TcpListener::bind(("127.0.0.1", PORT)).ok()
}

/// Events reach the UI thread as a boxed `Event` in LPARAM of WM_APP; the receiver owns it.
pub fn spawn(listener: TcpListener, hwnd: HWND) {
    let target = hwnd.0 as isize;
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let Some(event) = handle(stream) else { continue };
            let ptr = Box::into_raw(Box::new(event));
            let posted = unsafe {
                PostMessageW(Some(HWND(target as *mut _)), WM_APP, WPARAM(0), LPARAM(ptr as isize))
            };
            if posted.is_err() {
                drop(unsafe { Box::from_raw(ptr) });
            }
        }
    });
}

const MAX_STDIN: u64 = 64 * 1024;
const STDIN_TIMEOUT: Duration = Duration::from_millis(300);

/// Value of a top-level-looking `"key": "string"` in JSON text. Not a real parser: hook payloads are
/// flat and we only need two strings, so this avoids a JSON dependency.
fn json_str(text: &str, key: &str) -> Option<String> {
    let pat = format!("\"{key}\"");
    let after = &text[text.find(&pat)? + pat.len()..];
    let rest = after.trim_start().strip_prefix(':')?.trim_start().strip_prefix('"')?;
    let mut out = String::new();
    let mut chars = rest.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(out),
            '\\' => match chars.next()? {
                'n' | 't' | 'r' => out.push(' '),
                other => out.push(other), // covers \\ \" \/
            },
            c => out.push(c),
        }
    }
    None
}

fn folder_name(path: &str) -> String {
    path.trim_end_matches(['\\', '/']).rsplit(['\\', '/']).next().unwrap_or("").to_string()
}

/// One short line describing a tool call from a hook payload, e.g. "Running cargo build" or "Editing main.rs".
/// Looks the fields up by name anywhere in the payload (flat lookups are enough for what we show).
pub fn summarize_tool(payload: &str) -> String {
    let tool = json_str(payload, "tool_name").unwrap_or_default();
    let get = |key: &str| json_str(payload, key).unwrap_or_default();
    let file = || {
        let path = json_str(payload, "file_path").or_else(|| json_str(payload, "notebook_path")).unwrap_or_default();
        folder_name(&path)
    };
    let line = match tool.as_str() {
        "" => String::new(),
        "Bash" => format!("Running {}", get("command")),
        "Edit" | "MultiEdit" | "NotebookEdit" => format!("Editing {}", file()),
        "Write" => format!("Writing {}", file()),
        "Read" => format!("Reading {}", file()),
        "Grep" | "Glob" => format!("Searching {}", get("pattern")),
        "WebFetch" => format!("Fetching {}", get("url")),
        "WebSearch" => format!("Searching the web: {}", get("query")),
        "Task" | "Agent" => format!("Subagent: {}", get("description")),
        other => other.to_string(), // anything else (e.g. an MCP tool): just its name
    };
    line.trim().to_string()
}

/// Agent hooks pipe a JSON payload to stdin. Read it (bounded, with a timeout so a never-closed pipe
/// can't hang the hook) and pull out the session id, project folder, and the detail for this status.
fn hook_context(status: &str) -> (String, String, String) {
    use std::io::IsTerminal;
    let mut payload = String::new();
    if !std::io::stdin().is_terminal() {
        let (tx, rx) = std::sync::mpsc::channel();
        thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = std::io::stdin().take(MAX_STDIN).read_to_end(&mut buf);
            let _ = tx.send(String::from_utf8_lossy(&buf).into_owned());
        });
        payload = rx.recv_timeout(STDIN_TIMEOUT).unwrap_or_default();
    }
    // Claude Code exports the project root; its payload `cwd` may be a subfolder.
    let dir = std::env::var("CLAUDE_PROJECT_DIR")
        .ok()
        .filter(|d| !d.is_empty())
        .or_else(|| json_str(&payload, "cwd"))
        .unwrap_or_default();
    let detail = match status {
        "working" => json_str(&payload, "prompt").unwrap_or_default(), // the task you just sent
        "activity" => summarize_tool(&payload),
        _ => String::new(),
    };
    (
        clean(&json_str(&payload, "session_id").unwrap_or_default()),
        clean(&folder_name(&dir)),
        clean_n(&detail, MAX_DETAIL),
    )
}

/// CLI: `notch <working|activity|done|attention|clear> [title] [--agent name]`. Returns the process exit code.
pub fn send_from_args(args: &[String]) -> i32 {
    if parse(&format!("a\t{}\t", args[0])).is_none() {
        return 2; // not a status we know: report misuse even when the notch is off
    }
    if !notch_is_running() {
        return 0; // normal state for agent hooks: nothing to tell, and no time wasted on a refused connection
    }
    let mut agent = "agent".to_string();
    let mut title = String::new();
    let mut it = args[1..].iter();
    while let Some(a) = it.next() {
        if a == "--agent" {
            if let Some(v) = it.next() { agent = v.clone() }
        } else if title.is_empty() {
            title = a.clone();
        }
    }
    // The foreground window now is normally the terminal the user is working in.
    let hwnd = unsafe { GetForegroundWindow() }.0 as isize;
    let (session, project, detail) = hook_context(&args[0]);
    let title = title.replace(['\t', '\r', '\n'], " ");
    let line = format!("{agent}\t{}\t{title}\t{hwnd}\t{session}\t{project}\t{detail}\n", args[0]);
    if parse(&line).is_none() {
        return 2;
    }
    let addr = ([127, 0, 0, 1], PORT).into();
    // Notch not running is a normal state (e.g. when called from agent hooks): stay silent, exit 0.
    let _ = TcpStream::connect_timeout(&addr, IO_TIMEOUT).and_then(|mut s| s.write_all(line.as_bytes()));
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_the_app_a_window_belongs_to() {
        assert_eq!(friendly_app(r"C:\Users\x\AppData\Local\Programs\Microsoft VS Code\Code.exe"), "VS Code");
        assert_eq!(friendly_app(r"C:\Windows\System32\WindowsTerminal.exe"), "Terminal");
        assert_eq!(friendly_app(r"C:\Program Files\Google\Chrome\Application\chrome.exe"), "Chrome");
        assert_eq!(friendly_app(r"D:\tools\antigravity.exe"), "Antigravity"); // unknown apps: capitalised exe name
        assert_eq!(friendly_app(""), "");
        assert_eq!(place_of(0, "Gemini"), "Chrome"); // browser agents have no window
        assert_eq!(place_of(0, "claude"), "");
    }

    #[test]
    fn parses_valid_line() {
        let e = parse("claude\tdone\tRefactor finished\n").unwrap();
        assert_eq!((e.agent.as_str(), e.status, e.title.as_str()), ("claude", Status::Done, "Refactor finished"));
    }

    #[test]
    fn parses_activity_with_detail() {
        let e = parse("claude\tactivity\t\t0\ts1\tapp\tRunning cargo build").unwrap();
        assert_eq!((e.status, e.detail.as_str(), e.key().as_str()), (Status::Activity, "Running cargo build", "claude#s1"));
        assert_eq!(parse("claude\tworking\tx").unwrap().detail, ""); // detail is optional
        assert_eq!(parse(&format!("a\tworking\t\t0\t\t\t{}", "x".repeat(500))).unwrap().detail.len(), MAX_DETAIL);
    }

    #[test]
    fn summarizes_tool_calls() {
        let bash = r#"{"session_id":"s","tool_name":"Bash","tool_input":{"command":"cargo build --release","description":"Build"}}"#;
        assert_eq!(summarize_tool(bash), "Running cargo build --release");
        let edit = r#"{"tool_name":"Edit","tool_input":{"file_path":"E:\\Project Notch\\src\\main.rs","old_string":"a"}}"#;
        assert_eq!(summarize_tool(edit), "Editing main.rs");
        assert_eq!(summarize_tool(r#"{"tool_name":"Read","tool_input":{"file_path":"C:/a/b/notes.md"}}"#), "Reading notes.md");
        assert_eq!(summarize_tool(r#"{"tool_name":"Grep","tool_input":{"pattern":"fn main"}}"#), "Searching fn main");
        assert_eq!(summarize_tool(r#"{"tool_name":"mcp__pencil__execute"}"#), "mcp__pencil__execute");
        assert_eq!(summarize_tool(r#"{"hook_event_name":"Stop"}"#), ""); // no tool: nothing to show
        assert_eq!(summarize_tool("not json"), "");
    }

    #[test]
    fn parses_clear_status() {
        let e = parse("codex\tclear\t").unwrap();
        assert_eq!((e.agent.as_str(), e.status), ("codex", Status::Clear));
    }

    #[test]
    fn title_is_optional() {
        assert_eq!(parse("claude\tattention\t").unwrap().title, "");
        assert_eq!(parse("claude\tworking").unwrap().status, Status::Working);
    }

    #[test]
    fn parses_optional_hwnd() {
        assert_eq!(parse("a\tdone\tx\t4242\n").unwrap().hwnd, 4242);
        assert_eq!(parse("a\tdone\tx\tnot-a-number").unwrap().hwnd, 0);
        assert_eq!(parse("a\tdone\tx").unwrap().hwnd, 0);
    }

    #[test]
    fn parses_session_and_project_and_builds_keys() {
        let e = parse("claude\tdone\tx\t0\tabc-123\tmy-app").unwrap();
        assert_eq!((e.session.as_str(), e.project.as_str(), e.key().as_str()), ("abc-123", "my-app", "claude#abc-123"));
        assert_eq!(parse("claude\tdone\tx").unwrap().key(), "claude"); // no session: whole agent is one run
    }

    #[test]
    fn reads_hook_payload_fields() {
        let json = r#"{"session_id":"s-1","cwd":"E:\\Project Notch\\src","hook_event_name":"Stop"}"#;
        assert_eq!(json_str(json, "session_id").as_deref(), Some("s-1"));
        assert_eq!(json_str(json, "cwd").as_deref(), Some(r"E:\Project Notch\src"));
        assert_eq!(json_str(json, "missing"), None);
        assert_eq!(json_str(r#"{"cwd": "C:/a/b"}"#, "cwd").as_deref(), Some("C:/a/b"));
    }

    #[test]
    fn folder_name_handles_slashes_and_trailing_separators() {
        assert_eq!(folder_name(r"E:\Project Notch\"), "Project Notch");
        assert_eq!(folder_name("/home/me/app"), "app");
        assert_eq!(folder_name(""), "");
    }

    #[test]
    fn rejects_bad_status_and_empty_agent() {
        assert!(parse("claude\tfinished\tx").is_none());
        assert!(parse("\tdone\tx").is_none());
        assert!(parse("garbage").is_none());
    }

    #[test]
    fn strips_control_chars_and_caps_length() {
        let long = "a".repeat(500);
        let e = parse(&format!("ag\u{7}ent\tdone\t{long}")).unwrap();
        assert_eq!(e.agent, "agent");
        assert_eq!(e.title.len(), MAX_FIELD);
    }

    #[test]
    fn extracts_http_body() {
        let req = "POST /e HTTP/1.1\r\nHost: x\r\n\r\nclaude\tdone\thi";
        assert_eq!(parse(payload_of(req)).unwrap().title, "hi");
    }
}
