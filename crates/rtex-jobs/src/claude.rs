//! Calls the Claude Code CLI on this Mac (your own Claude login) as a plain
//! question/answer: no tools, no MCP servers, no project settings, a custom
//! system prompt and a JSON Schema for the answer. That keeps each call
//! close to a web-app chat and keeps the overhead to about 1K tokens.
//!
//! The answer is streamed (`--output-format stream-json
//! --include-partial-messages`): structured output arrives as a
//! `StructuredOutput` tool call whose JSON grows chunk by chunk, so every
//! finished edit is reported the moment its closing brace arrives.

use serde::Serialize;
use serde_json::Value;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::Instant;

#[derive(Debug, Clone, Default)]
pub struct Options {
    pub model: Option<String>,
    pub effort: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Reply {
    #[serde(skip)]
    pub value: Value,
    pub cost_usd: f64,
    pub seconds: f64,
    pub output_tokens: u64,
    pub input_tokens: u64,
    /// Input served from the prompt cache (background reused across jobs).
    pub cached_input_tokens: u64,
    pub model: String,
}

/// What the stream reports while Claude works.
#[derive(Debug, Clone)]
pub enum Event {
    /// Thinking (extended reasoning) is under way.
    Thinking,
    /// The answer has started arriving.
    Writing,
    /// One complete element of the answer's `edits` array (0-based index).
    Edit(usize, Value),
}

/// The `claude` executable: `$RTEX_CLAUDE`, else `$PATH`, else what a login
/// shell finds (GUI apps don't inherit nvm/Homebrew PATH entries).
pub fn binary() -> Option<PathBuf> {
    static FOUND: OnceLock<Option<PathBuf>> = OnceLock::new();
    FOUND
        .get_or_init(|| {
            if let Some(p) = std::env::var_os("RTEX_CLAUDE") {
                return Some(PathBuf::from(p));
            }
            if let Some(path) = std::env::var_os("PATH") {
                for d in std::env::split_paths(&path) {
                    let c = d.join("claude");
                    if c.is_file() {
                        return Some(c);
                    }
                }
            }
            let out = Command::new("/bin/zsh").args(["-lic", "command -v claude"]).output().ok()?;
            let p = String::from_utf8_lossy(&out.stdout).lines().last()?.trim().to_string();
            (!p.is_empty() && PathBuf::from(&p).is_file()).then(|| PathBuf::from(p))
        })
        .clone()
}

/// Ask once, streaming. `user` goes in on stdin; the answer must match `schema`.
pub fn ask(opts: &Options, system: &str, user: &str, schema: &Value, on_event: &mut dyn FnMut(Event)) -> Result<Reply, String> {
    let t0 = Instant::now();
    let bin = binary().ok_or("Claude Code CLI not found (install it, or set RTEX_CLAUDE to its path)")?;
    // An empty working folder: no CLAUDE.md or project files get picked up.
    let work = std::env::temp_dir().join(format!("rtex-claude-{}", std::process::id()));
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    static CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let sys_file = work.join(format!("system-{n}.md"));
    std::fs::write(&sys_file, system).map_err(|e| e.to_string())?;

    let mut cmd = Command::new(&bin);
    cmd.current_dir(&work)
        .args(["-p", "--output-format", "stream-json", "--verbose", "--include-partial-messages"])
        .arg("--no-session-persistence")
        .args(["--tools", ""])
        .args(["--strict-mcp-config", "--mcp-config", r#"{"mcpServers":{}}"#])
        .args(["--setting-sources", ""])
        .arg("--system-prompt-file")
        .arg(&sys_file)
        .arg("--json-schema")
        .arg(schema.to_string());
    if let Some(m) = opts.model.as_deref().filter(|m| !m.is_empty()) {
        cmd.args(["--model", m]);
    }
    if let Some(e) = opts.effort.as_deref().filter(|e| !e.is_empty()) {
        cmd.args(["--effort", e]);
    }
    // GUI launches have a minimal PATH; give the CLI (a Node script) one that finds node.
    if let Some(dir) = bin.parent() {
        let path = std::env::var("PATH").unwrap_or_default();
        cmd.env("PATH", format!("{}:{path}:/usr/bin:/bin:/opt/homebrew/bin:/usr/local/bin", dir.display()));
    }
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot start {}: {e}", bin.display()))?;
    {
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(user.as_bytes()).map_err(|e| e.to_string())?;
    }
    let mut stderr = child.stderr.take().unwrap();
    let err_thread = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stderr.read_to_string(&mut s);
        s
    });

    let mut partial = PartialEdits::default();
    let mut result: Option<Value> = None;
    let (mut said_thinking, mut said_writing) = (false, false);
    for line in BufReader::new(child.stdout.take().unwrap()).lines() {
        let Ok(line) = line else { break };
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        match v["type"].as_str() {
            Some("stream_event") => {
                let ev = &v["event"];
                match ev["delta"]["type"].as_str() {
                    Some("thinking_delta") if !said_thinking => {
                        said_thinking = true;
                        on_event(Event::Thinking);
                    }
                    Some("input_json_delta") => {
                        if !said_writing {
                            said_writing = true;
                            on_event(Event::Writing);
                        }
                        let chunk = ev["delta"]["partial_json"].as_str().unwrap_or("");
                        for (i, e) in partial.push(chunk) {
                            on_event(Event::Edit(i, e));
                        }
                    }
                    _ => {}
                }
            }
            Some("result") => result = Some(v),
            _ => {}
        }
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    let stderr_text = err_thread.join().unwrap_or_default();
    let _ = std::fs::remove_file(&sys_file);
    let v = result.ok_or_else(|| format!("Claude Code ended without an answer (exit {status}): {}", stderr_text.trim()))?;
    if v["is_error"].as_bool().unwrap_or(false) {
        return Err(format!("Claude Code error: {}", v["result"].as_str().unwrap_or("unknown")));
    }
    let value = match &v["structured_output"] {
        Value::Null => serde_json::from_str(v["result"].as_str().unwrap_or("")).map_err(|e| format!("answer was not JSON: {e}"))?,
        s => s.clone(),
    };
    let u = &v["usage"];
    Ok(Reply {
        value,
        cost_usd: v["total_cost_usd"].as_f64().unwrap_or(0.0),
        seconds: t0.elapsed().as_secs_f64(),
        output_tokens: u["output_tokens"].as_u64().unwrap_or(0),
        input_tokens: u["input_tokens"].as_u64().unwrap_or(0)
            + u["cache_read_input_tokens"].as_u64().unwrap_or(0)
            + u["cache_creation_input_tokens"].as_u64().unwrap_or(0),
        cached_input_tokens: u["cache_read_input_tokens"].as_u64().unwrap_or(0),
        model: v["modelUsage"].as_object().and_then(|m| m.keys().next().cloned()).unwrap_or_default(),
    })
}

/// Finds complete objects in the `"edits": [...]` array of JSON that is
/// still arriving.
#[derive(Default)]
pub struct PartialEdits {
    buf: String,
    /// Byte offset just inside `[` of the edits array, once seen.
    array_start: Option<usize>,
    /// Scan position and parser state inside the array.
    pos: usize,
    depth: usize,
    in_str: bool,
    escaped: bool,
    obj_start: usize,
    emitted: usize,
    done: bool,
}

impl PartialEdits {
    /// Add a chunk; returns edits completed by it.
    pub fn push(&mut self, chunk: &str) -> Vec<(usize, Value)> {
        self.buf.push_str(chunk);
        let mut out = Vec::new();
        if self.done {
            return out;
        }
        if self.array_start.is_none() {
            let Some(k) = self.buf.find("\"edits\"") else { return out };
            let Some(b) = self.buf[k..].find('[') else { return out };
            self.array_start = Some(k + b + 1);
            self.pos = k + b + 1;
        }
        let bytes = self.buf.as_bytes();
        while self.pos < bytes.len() {
            let c = bytes[self.pos];
            if self.in_str {
                if self.escaped {
                    self.escaped = false;
                } else if c == b'\\' {
                    self.escaped = true;
                } else if c == b'"' {
                    self.in_str = false;
                }
            } else {
                match c {
                    b'"' => self.in_str = true,
                    b'{' => {
                        if self.depth == 0 {
                            self.obj_start = self.pos;
                        }
                        self.depth += 1;
                    }
                    b'}' => {
                        self.depth = self.depth.saturating_sub(1);
                        if self.depth == 0 {
                            if let Ok(v) = serde_json::from_str::<Value>(&self.buf[self.obj_start..=self.pos]) {
                                out.push((self.emitted, v));
                                self.emitted += 1;
                            }
                        }
                    }
                    b']' if self.depth == 0 => {
                        self.done = true;
                        self.pos += 1;
                        break;
                    }
                    _ => {}
                }
            }
            self.pos += 1;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_arrive_one_by_one() {
        let json = r#"{"decision":"tailor","edits":[{"op":"replace","original":"a } { \" ]","replacement":"b"},{"op":"delete","original":"c","replacement":""}],"x":1}"#;
        let mut p = PartialEdits::default();
        let mut got = Vec::new();
        for chunk in json.as_bytes().chunks(7) {
            for (i, v) in p.push(std::str::from_utf8(chunk).unwrap()) {
                got.push((i, v["op"].as_str().unwrap().to_string()));
            }
        }
        assert_eq!(got, vec![(0, "replace".to_string()), (1, "delete".to_string())]);
    }
}
