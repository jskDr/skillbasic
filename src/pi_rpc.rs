use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const RPC_TIMEOUT: Duration = Duration::from_secs(120);

enum RpcRead {
    Line(String),
    Error(String),
    Eof,
}

pub struct PiRpcClient {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: Receiver<RpcRead>,
    reader: Option<JoinHandle<()>>,
    next_id: u64,
    timeout: Duration,
    stopped: bool,
}

impl PiRpcClient {
    pub fn spawn(config: &crate::ai::PiConfig) -> Result<Self, String> {
        let mut command = Command::new("pi");
        command.args(pi_args(config));
        Self::spawn_command(command, RPC_TIMEOUT)
    }

    fn spawn_command(mut command: Command, timeout: Duration) -> Result<Self, String> {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| format!("?AI ERROR: failed to spawn pi: {}", e))?;
        let Some(stdin) = child.stdin.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return Err("?AI ERROR: failed to open pi stdin".into());
        };
        let Some(stdout) = child.stdout.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return Err("?AI ERROR: failed to open pi stdout".into());
        };

        let (tx, rx) = mpsc::channel();
        let reader = thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut line = String::new();
                match reader.read_line(&mut line) {
                    Ok(0) => {
                        let _ = tx.send(RpcRead::Eof);
                        break;
                    }
                    Ok(_) => {
                        if line.ends_with('\n') {
                            line.pop();
                            if line.ends_with('\r') {
                                line.pop();
                            }
                        }
                        if tx.send(RpcRead::Line(line)).is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(RpcRead::Error(e.to_string()));
                        break;
                    }
                }
            }
        });

        Ok(PiRpcClient {
            child,
            stdin: Some(stdin),
            stdout: rx,
            reader: Some(reader),
            next_id: 1,
            timeout,
            stopped: false,
        })
    }

    pub fn prompt(&mut self, message: &str) -> Result<String, String> {
        self.prompt_stream(message, |_| Ok(()))
    }

    pub fn prompt_stream<F>(&mut self, message: &str, mut on_text: F) -> Result<String, String>
    where
        F: FnMut(&str) -> Result<(), String>,
    {
        let id = self.send_command("prompt", serde_json::json!({"message": message}))?;
        let deadline = Instant::now() + self.timeout;
        let mut accepted = false;
        let mut response_text = String::new();

        loop {
            let value = self.next_value(deadline)?;
            match value.get("type").and_then(|t| t.as_str()) {
                Some("response") if value.get("id").and_then(|v| v.as_str()) == Some(&id) => {
                    self.check_response(&value, "prompt")?;
                    accepted = true;
                }
                Some("message_update") => {
                    if let Some(event) = value.get("assistantMessageEvent") {
                        match event.get("type").and_then(|t| t.as_str()) {
                            Some("text_delta") => {
                                if let Some(text) = event.get("delta").and_then(|d| d.as_str()) {
                                    response_text.push_str(text);
                                    if let Err(e) = on_text(text) {
                                        return self.fail(&e);
                                    }
                                }
                            }
                            Some("error") => {
                                let detail = event
                                    .get("error")
                                    .and_then(|e| e.as_str())
                                    .unwrap_or("pi agent failed");
                                return Err(format!("?AI ERROR: {}", detail));
                            }
                            _ => {}
                        }
                    }
                }
                Some("agent_settled") => {
                    if !accepted {
                        return self.fail("?AI ERROR: pi settled without accepting the prompt");
                    }
                    if response_text.is_empty() {
                        return Err("?AI ERROR: empty response from pi".into());
                    }
                    return Ok(response_text);
                }
                _ => {}
            }
        }
    }

    pub fn new_session(&mut self) -> Result<(), String> {
        let id = self.send_command("new_session", serde_json::json!({}))?;
        let deadline = Instant::now() + self.timeout;
        loop {
            let value = self.next_value(deadline)?;
            if value.get("type").and_then(|t| t.as_str()) == Some("response")
                && value.get("id").and_then(|v| v.as_str()) == Some(&id)
            {
                self.check_response(&value, "new_session")?;
                if value.pointer("/data/cancelled").and_then(|v| v.as_bool()) == Some(true) {
                    return Err("?AI ERROR: pi session reset was cancelled".into());
                }
                return Ok(());
            }
        }
    }

    fn send_command(&mut self, command: &str, fields: serde_json::Value) -> Result<String, String> {
        if self.stopped {
            return Err("?AI ERROR: pi RPC client is no longer running".into());
        }
        let id = format!("skillbasic-{}", self.next_id);
        self.next_id += 1;
        let mut value = fields;
        let object = value
            .as_object_mut()
            .ok_or("?AI ERROR: invalid internal pi RPC command")?;
        object.insert("id".into(), serde_json::Value::String(id.clone()));
        object.insert("type".into(), serde_json::Value::String(command.into()));

        let Some(stdin) = self.stdin.as_mut() else {
            return Err("?AI ERROR: pi RPC stdin is closed".into());
        };
        if let Err(e) = writeln!(stdin, "{}", value).and_then(|_| stdin.flush()) {
            let message = format!("?AI ERROR: write to pi failed: {}", e);
            return self.fail(&message);
        }
        Ok(id)
    }

    fn next_value(&mut self, deadline: Instant) -> Result<serde_json::Value, String> {
        loop {
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                return self.fail("?AI ERROR: pi RPC timed out");
            };
            match self.stdout.recv_timeout(remaining) {
                Ok(RpcRead::Line(line)) if line.trim().is_empty() => continue,
                Ok(RpcRead::Line(line)) => match serde_json::from_str(&line) {
                    Ok(value) => return Ok(value),
                    Err(e) => return self.fail(&format!("?AI ERROR: bad JSON from pi: {}", e)),
                },
                Ok(RpcRead::Error(e)) => {
                    return self.fail(&format!("?AI ERROR: read from pi failed: {}", e));
                }
                Ok(RpcRead::Eof) => {
                    return self.fail("?AI ERROR: pi RPC process closed its output");
                }
                Err(RecvTimeoutError::Timeout) => {
                    return self.fail("?AI ERROR: pi RPC timed out");
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return self.fail("?AI ERROR: pi RPC reader stopped unexpectedly");
                }
            }
        }
    }

    fn check_response(&mut self, value: &serde_json::Value, command: &str) -> Result<(), String> {
        if value.get("success").and_then(|v| v.as_bool()) == Some(true) {
            return Ok(());
        }
        let error = value
            .get("error")
            .and_then(|e| e.as_str())
            .unwrap_or("unknown error");
        self.fail(&format!("?AI ERROR: pi rejected {}: {}", command, error))
    }

    fn fail<T>(&mut self, message: &str) -> Result<T, String> {
        self.stop();
        Err(message.to_string())
    }

    fn stop(&mut self) {
        if self.stopped {
            return;
        }
        self.stopped = true;
        self.stdin.take();
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

impl Drop for PiRpcClient {
    fn drop(&mut self) {
        self.stop();
    }
}

fn pi_args(config: &crate::ai::PiConfig) -> Vec<String> {
    let mut args = vec!["--mode".into(), "rpc".into(), "--no-session".into()];
    if !config.tools {
        args.push("--no-tools".into());
        args.push("--no-extensions".into());
    }
    if let Some(model) = &config.model {
        args.push("--model".into());
        args.push(model.clone());
    }
    if let Some(system) = &config.system {
        args.push("--system-prompt".into());
        args.push(system.clone());
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shell_client(script: &str, timeout: Duration) -> PiRpcClient {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", script]);
        PiRpcClient::spawn_command(command, timeout).unwrap()
    }

    #[test]
    fn pi_is_started_without_tools_by_default() {
        let args = pi_args(&crate::ai::PiConfig::default());
        assert!(args.iter().any(|arg| arg == "--no-tools"));
        assert!(args.iter().any(|arg| arg == "--no-extensions"));
    }

    #[test]
    fn pi_options_are_forwarded() {
        let config = crate::ai::PiConfig {
            model: Some("provider/model".into()),
            system: Some("Be concise".into()),
            tools: true,
        };
        let args = pi_args(&config);
        assert!(!args.iter().any(|arg| arg == "--no-tools"));
        assert!(!args.iter().any(|arg| arg == "--no-extensions"));
        assert!(args.windows(2).any(|a| a == ["--model", "provider/model"]));
        assert!(
            args.windows(2)
                .any(|a| a == ["--system-prompt", "Be concise"])
        );
    }

    #[test]
    fn prompt_collects_streamed_text() {
        let script = r#"read request
printf '%s\n' '{"id":"skillbasic-1","type":"response","command":"prompt","success":true}'
printf '%s\n' '{"type":"message_update","assistantMessageEvent":{"type":"text_delta","delta":"hello "}}'
printf '%s\n' '{"type":"message_update","assistantMessageEvent":{"type":"text_delta","delta":"world"}}'
printf '%s\n' '{"type":"agent_settled"}'
sleep 1"#;
        let mut client = shell_client(script, Duration::from_secs(1));
        assert_eq!(client.prompt("test").unwrap(), "hello world");
    }

    #[test]
    fn prompt_forwards_text_deltas_while_collecting_response() {
        let script = r#"read request
printf '%s\n' '{"id":"skillbasic-1","type":"response","command":"prompt","success":true}'
printf '%s\n' '{"type":"message_update","assistantMessageEvent":{"type":"text_delta","delta":"안녕"}}'
printf '%s\n' '{"type":"message_update","assistantMessageEvent":{"type":"text_delta","delta":"하세요"}}'
printf '%s\n' '{"type":"agent_settled"}'
sleep 1"#;
        let mut client = shell_client(script, Duration::from_secs(1));
        let mut deltas = Vec::new();
        let reply = client
            .prompt_stream("test", |text| {
                deltas.push(text.to_string());
                Ok(())
            })
            .unwrap();
        assert_eq!(deltas, ["안녕", "하세요"]);
        assert_eq!(reply, "안녕하세요");
    }

    #[test]
    fn rejected_prompt_is_an_error() {
        let script = r#"read request
printf '%s\n' '{"id":"skillbasic-1","type":"response","command":"prompt","success":false,"error":"rejected"}'
sleep 1"#;
        let mut client = shell_client(script, Duration::from_secs(1));
        let error = client.prompt("test").unwrap_err();
        assert!(error.contains("rejected"), "got: {}", error);
    }

    #[test]
    fn unresponsive_process_times_out() {
        let script = "read request; exec sleep 1";
        let mut client = shell_client(script, Duration::from_millis(50));
        let start = Instant::now();
        let error = client.prompt("test").unwrap_err();
        assert!(error.contains("timed out"), "got: {}", error);
        assert!(start.elapsed() < Duration::from_millis(500));
    }

    #[test]
    fn malformed_json_stops_the_client() {
        let script = "read request; printf '%s\\n' 'not-json'";
        let mut client = shell_client(script, Duration::from_secs(1));
        let error = client.prompt("test").unwrap_err();
        assert!(error.contains("bad JSON"), "got: {}", error);
        assert!(
            client
                .prompt("again")
                .unwrap_err()
                .contains("no longer running")
        );
    }

    #[test]
    fn new_session_waits_for_its_response() {
        let script = r#"read request
printf '%s\n' '{"id":"skillbasic-1","type":"response","command":"new_session","success":true,"data":{"cancelled":false}}'
sleep 1"#;
        let mut client = shell_client(script, Duration::from_secs(1));
        client.new_session().unwrap();
    }

    #[test]
    fn prompt_after_session_reset_uses_the_next_request_id() {
        let script = r#"read reset
printf '%s\n' '{"id":"skillbasic-1","type":"response","command":"new_session","success":true,"data":{"cancelled":false}}'
read prompt
printf '%s\n' '{"id":"skillbasic-2","type":"response","command":"prompt","success":true}'
printf '%s\n' '{"type":"message_update","assistantMessageEvent":{"type":"text_delta","delta":"fresh"}}'
printf '%s\n' '{"type":"agent_settled"}'
sleep 1"#;
        let mut client = shell_client(script, Duration::from_secs(1));
        client.new_session().unwrap();
        assert_eq!(client.prompt("test").unwrap(), "fresh");
    }
}
