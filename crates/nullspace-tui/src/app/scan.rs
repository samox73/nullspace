use std::collections::HashSet;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;

use nullspace_core::{Equation, Quantity, QuantityId};
use serde_json::json;

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanAgent {
    Claude,
    Codex,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanModel {
    ClaudeOpus,
    ClaudeSonnet,
    CodexGpt55,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanEffort {
    Low,
    Medium,
    High,
    XHigh,
    Max,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanPhase {
    AwaitingPaste,
    Running,
    Failed,
}

pub enum ScanEvent {
    Log(String),
    Done(Result<Box<ScanResult>, String>),
}

pub struct ScanResult {
    pub equation: Equation,
    pub new_quantities: Vec<Quantity>,
}

pub struct ScanState {
    pub agent: ScanAgent,
    pub model: ScanModel,
    pub effort: ScanEffort,
    pub phase: ScanPhase,
    pub workdir: PathBuf,
    pub image_path: Option<PathBuf>,
    pub logs: Vec<String>,
    pub rx: Option<mpsc::Receiver<ScanEvent>>,
    pub child: Option<std::process::Child>,
    pub staged_quantities: Vec<Quantity>,
}

impl AppState {
    pub fn start_scan(&mut self, agent: ScanAgent) {
        let workdir = std::env::temp_dir().join(format!("nullspace-scan-{}", std::process::id()));
        self.clear_scan();
        if let Err(err) = std::fs::create_dir_all(&workdir) {
            self.report_error(err);
            return;
        }
        self.scan = Some(ScanState {
            agent,
            model: ScanModel::for_agent(agent),
            effort: ScanEffort::for_agent(agent),
            phase: ScanPhase::AwaitingPaste,
            workdir,
            image_path: None,
            logs: Vec::new(),
            rx: None,
            child: None,
            staged_quantities: Vec::new(),
        });
        self.mode = Mode::Scan;
        self.status = "Copy an equation image, then press p".to_string();
    }

    pub(super) fn scan_paste(&mut self) {
        let Some(scan) = &mut self.scan else {
            return;
        };
        let image = match arboard::Clipboard::new().and_then(|mut clipboard| clipboard.get_image())
        {
            Ok(image) => image,
            Err(_) => {
                self.status = "clipboard has no image".to_string();
                scan.phase = ScanPhase::AwaitingPaste;
                return;
            }
        };
        let Some(rgba) = image::RgbaImage::from_raw(
            image.width as u32,
            image.height as u32,
            image.bytes.into_owned(),
        ) else {
            self.status = "clipboard image has unsupported format".to_string();
            scan.phase = ScanPhase::AwaitingPaste;
            return;
        };
        let path = scan.workdir.join("capture.png");
        if let Err(err) = rgba.save(&path) {
            self.status = err.to_string();
            self.notification = Some(Notification::error(self.status.clone()));
            scan.phase = ScanPhase::Failed;
            return;
        }
        scan.image_path = Some(path);
        self.spawn_scan();
    }

    pub(super) fn spawn_scan(&mut self) {
        let Some(scan) = &mut self.scan else {
            return;
        };
        let Some(image_path) = scan.image_path.clone() else {
            self.status = "paste an image first".to_string();
            return;
        };
        kill_child(scan);

        let existing = match self.store.quantities() {
            Ok(quantities) => quantities
                .into_iter()
                .map(|(quantity, _)| quantity)
                .collect::<Vec<_>>(),
            Err(err) => {
                self.report_error(err);
                return;
            }
        };
        let existing_ids = existing
            .iter()
            .map(|quantity| quantity.id)
            .collect::<HashSet<_>>();
        let schema = crate::export_file_schema();
        let schema_json = match serde_json::to_string_pretty(&schema) {
            Ok(json) => json,
            Err(err) => {
                self.report_error(err);
                return;
            }
        };
        let schema_path = scan.workdir.join("schema.json");
        if let Err(err) = std::fs::write(&schema_path, &schema_json) {
            self.report_error(err);
            return;
        }
        let image_name = image_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("capture.png");
        let prompt = build_prompt(image_name, &schema_json, &existing, scan.model.agent());

        let mut command = match scan.model.agent() {
            ScanAgent::Claude => {
                let mut command = Command::new("claude");
                let model = scan.model.cli_model();
                let effort = scan.effort.claude_value();
                command.args([
                    "-p",
                    &prompt,
                    "--model",
                    model,
                    "--effort",
                    effort,
                    "--output-format",
                    "stream-json",
                    "--verbose",
                    "--allowedTools",
                    "Read,Write",
                ]);
                command
            }
            ScanAgent::Codex => {
                let mut command = Command::new("codex");
                let model = scan.model.cli_model();
                let effort_config = format!(
                    "model_reasoning_effort={}",
                    scan.effort.codex_value().unwrap_or("high")
                );
                command.args([
                    "exec",
                    "-i",
                    "capture.png",
                    "-m",
                    model,
                    "-c",
                    &effort_config,
                    "--output-schema",
                    "schema.json",
                    "-o",
                    "result.json",
                    "--json",
                    "--skip-git-repo-check",
                    &prompt,
                ]);
                command
            }
        };
        let spawn_result = command
            .current_dir(&scan.workdir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();
        let Ok(mut child) = spawn_result else {
            let err = spawn_result.unwrap_err();
            scan.phase = ScanPhase::Failed;
            scan.logs.push(err.to_string());
            self.notification = Some(Notification::error(err.to_string()));
            return;
        };

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let (tx, rx) = mpsc::channel();
        scan.rx = Some(rx);
        scan.child = Some(child);
        scan.phase = ScanPhase::Running;
        scan.logs.clear();

        if let Some(stderr) = stderr {
            let tx = tx.clone();
            thread::spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    let _ = tx.send(ScanEvent::Log(line));
                }
            });
        }
        if let Some(stdout) = stdout {
            let workdir = scan.workdir.clone();
            thread::spawn(move || {
                for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                    if let Some(line) = readable_log_line(&line) {
                        let _ = tx.send(ScanEvent::Log(line));
                    }
                }
                let result = std::fs::read_to_string(workdir.join("result.json"))
                    .map_err(|_| "agent produced no result.json".to_string())
                    .and_then(|raw| parse_scan_result(&raw, &existing_ids).map(Box::new));
                let _ = tx.send(ScanEvent::Done(result));
            });
        }
    }

    pub(super) fn handle_scan_event(&mut self, event: ScanEvent) {
        match event {
            ScanEvent::Log(line) => {
                if let Some(scan) = &mut self.scan {
                    scan.logs.push(line);
                    let overflow = scan.logs.len().saturating_sub(500);
                    if overflow > 0 {
                        scan.logs.drain(..overflow);
                    }
                }
            }
            ScanEvent::Done(result) => {
                if let Some(scan) = &mut self.scan {
                    if let Some(mut child) = scan.child.take() {
                        let _ = child.wait();
                    }
                    scan.rx = None;
                }
                match result {
                    Ok(result) => {
                        if let Some(scan) = &mut self.scan {
                            scan.staged_quantities = result.new_quantities;
                        }
                        self.open_editor_with(Some(result.equation), None);
                        if let Some(editor) = &mut self.editor {
                            editor.last_saved_signature = String::new();
                        }
                        self.scan_review = true;
                    }
                    Err(msg) => {
                        if let Some(scan) = &mut self.scan {
                            scan.phase = ScanPhase::Failed;
                            scan.logs.push(msg.clone());
                        }
                        self.notification = Some(Notification::error(msg.clone()));
                        self.status = msg;
                    }
                }
            }
        }
    }

    pub(super) fn confirm_scan(&mut self) -> anyhow::Result<()> {
        let staged = self
            .scan
            .as_ref()
            .map(|scan| scan.staged_quantities.clone())
            .unwrap_or_default();
        // ponytail: staged quantities land before the equation; orphans are harmless and idempotent on retry
        self.store.import_quantities(&staged)?;
        self.persist_editor(true)?;
        if self.editor.is_none() {
            self.scan_review = false;
            self.clear_scan();
            self.notification = Some(Notification::info("equation added from scan"));
        }
        Ok(())
    }

    pub(super) fn rescan(&mut self) -> anyhow::Result<()> {
        let Some(scan) = &mut self.scan else {
            self.status = "No scan to retry".to_string();
            return Ok(());
        };
        if scan.image_path.is_none() {
            self.status = "Paste an image first".to_string();
            return Ok(());
        }
        kill_child(scan);
        self.editor = None;
        self.scan_review = false;
        self.mode = Mode::Scan;
        self.spawn_scan();
        Ok(())
    }

    pub(super) fn scan_cycle_model(&mut self) {
        let Some(scan) = &mut self.scan else {
            return;
        };
        if scan.phase == ScanPhase::Running {
            return;
        }
        scan.model = scan.model.next();
        scan.agent = scan.model.agent();
        scan.effort = scan.effort.supported_for(scan.model);
        self.status = scan.settings_label();
    }

    pub(super) fn scan_cycle_effort(&mut self) {
        let Some(scan) = &mut self.scan else {
            return;
        };
        if scan.phase == ScanPhase::Running {
            return;
        }
        scan.effort = scan.effort.next_for(scan.model);
        self.status = scan.settings_label();
    }

    pub(super) fn discard_scan(&mut self) {
        self.editor = None;
        self.scan_review = false;
        self.clear_scan();
        self.mode = Mode::Browser;
        self.notification = Some(Notification::info("scan discarded"));
        self.schedule_selected();
    }

    pub(super) fn back_from_scan(&mut self) {
        if let Some(scan) = &mut self.scan
            && scan.phase == ScanPhase::Running
        {
            kill_child(scan);
            scan.phase = ScanPhase::AwaitingPaste;
            scan.rx = None;
            self.status = "Scan cancelled".to_string();
            return;
        }
        self.clear_scan();
        self.mode = Mode::Browser;
        self.schedule_selected();
    }

    fn clear_scan(&mut self) {
        if let Some(mut scan) = self.scan.take() {
            kill_child(&mut scan);
            let _ = std::fs::remove_dir_all(scan.workdir);
        }
    }
}

impl ScanState {
    pub fn settings_label(&self) -> String {
        format!(
            "model: {} | intelligence: {}",
            self.model.label(),
            self.effort.label_for(self.model)
        )
    }
}

impl ScanModel {
    fn for_agent(agent: ScanAgent) -> Self {
        match agent {
            ScanAgent::Claude => Self::ClaudeOpus,
            ScanAgent::Codex => Self::CodexGpt55,
        }
    }

    fn agent(self) -> ScanAgent {
        match self {
            Self::ClaudeOpus | Self::ClaudeSonnet => ScanAgent::Claude,
            Self::CodexGpt55 => ScanAgent::Codex,
        }
    }

    fn next(self) -> Self {
        match self {
            Self::ClaudeOpus => Self::ClaudeSonnet,
            Self::ClaudeSonnet => Self::CodexGpt55,
            Self::CodexGpt55 => Self::ClaudeOpus,
        }
    }

    fn cli_model(self) -> &'static str {
        match self {
            Self::ClaudeOpus => "opus",
            Self::ClaudeSonnet => "sonnet",
            Self::CodexGpt55 => "gpt-5.5",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::ClaudeOpus => "opus",
            Self::ClaudeSonnet => "sonnet",
            Self::CodexGpt55 => "gpt-5.5",
        }
    }
}

impl ScanEffort {
    fn for_agent(agent: ScanAgent) -> Self {
        match agent {
            ScanAgent::Claude => Self::XHigh,
            ScanAgent::Codex => Self::High,
        }
    }

    fn next_for(self, model: ScanModel) -> Self {
        match model.agent() {
            ScanAgent::Claude => match self {
                Self::Low => Self::Medium,
                Self::Medium => Self::High,
                Self::High => Self::XHigh,
                Self::XHigh => Self::Max,
                Self::Max => Self::Low,
            },
            ScanAgent::Codex => match self.supported_for(model) {
                Self::Low => Self::Medium,
                Self::Medium => Self::High,
                Self::High => Self::XHigh,
                Self::XHigh | Self::Max => Self::Low,
            },
        }
    }

    fn supported_for(self, model: ScanModel) -> Self {
        match model.agent() {
            ScanAgent::Claude => self,
            ScanAgent::Codex => match self {
                Self::Low | Self::Medium | Self::High | Self::XHigh => self,
                Self::Max => Self::High,
            },
        }
    }

    fn claude_value(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::XHigh => "xhigh",
            Self::Max => "max",
        }
    }

    fn codex_value(self) -> Option<&'static str> {
        match self {
            Self::Low => Some("low"),
            Self::Medium => Some("medium"),
            Self::High => Some("high"),
            Self::XHigh => Some("extra_high"),
            Self::Max => None,
        }
    }

    fn label_for(self, model: ScanModel) -> &'static str {
        match (model.agent(), self) {
            (ScanAgent::Codex, Self::XHigh) => "extra high",
            (_, Self::Low) => "low",
            (_, Self::Medium) => "medium",
            (_, Self::High) => "high",
            (_, Self::XHigh) => "xhigh",
            (_, Self::Max) => "max",
        }
    }
}

fn kill_child(scan: &mut ScanState) {
    if let Some(mut child) = scan.child.take() {
        let _ = child.kill();
        let _ = child.wait();
    }
}

fn build_prompt(image: &str, schema: &str, quantities: &[Quantity], agent: ScanAgent) -> String {
    let quantities = quantities
        .iter()
        .map(|quantity| {
            json!({
                "id": quantity.id,
                "symbol": quantity.symbol,
                "name": quantity.name,
                "units": quantity.units,
            })
        })
        .collect::<Vec<_>>();
    let quantities = serde_json::to_string(&quantities).unwrap_or_else(|_| "[]".to_string());
    let closing = match agent {
        ScanAgent::Claude => {
            format!(
                "Read {image}, then use the Write tool to save ONLY the JSON to result.json in the working directory. No other output."
            )
        }
        ScanAgent::Codex => "Reply with ONLY the JSON as your final message.".to_string(),
    };
    format!(
        r#"You are given a screenshot of a physics equation: {image}.
Extract the PRIMARY equation and produce JSON with this exact shape
{{"quantities": [...], "equations": [...]}} conforming to this JSON schema:

{schema}

Rules:
- Exactly one entry in "equations": name, latex (valid LaTeX), description
  (1-2 concise sentences), assumptions (semicolon-separated validity
  conditions), tags, variables.
- For every variables[] entry set quantity_id: reuse an id from the existing
  quantities listed below when symbol/meaning match; otherwise append a new
  quantity to "quantities" (fresh UUIDv4 id, LaTeX symbol, name, description,
  units in SI or "dimensionless"). Leave quantity_id null for indices and
  dummy variables. Only include NEW quantities in "quantities".
- Leave a field empty rather than guessing. references: only if certain.

Existing quantities (id, symbol, name, units):
{quantities}

{closing}"#
    )
}

fn parse_scan_result(raw: &str, existing_ids: &HashSet<QuantityId>) -> Result<ScanResult, String> {
    let (quantities, equations) = crate::parse_import(raw).map_err(|err| err.to_string())?;
    let mut equation = equations
        .into_iter()
        .next()
        .ok_or_else(|| "scan result did not include an equation".to_string())?;
    if equation.name.trim().is_empty() || equation.latex.trim().is_empty() {
        return Err("scan result equation is missing name or latex".to_string());
    }
    let returned_ids = quantities
        .iter()
        .map(|quantity| quantity.id)
        .collect::<HashSet<_>>();
    let known_ids = existing_ids
        .union(&returned_ids)
        .copied()
        .collect::<HashSet<_>>();
    for variable in &mut equation.variables {
        if variable
            .quantity_id
            .is_some_and(|quantity_id| !known_ids.contains(&quantity_id))
        {
            variable.quantity_id = None;
        }
    }
    let new_quantities = quantities
        .into_iter()
        .filter(|quantity| !existing_ids.contains(&quantity.id))
        .collect();
    Ok(ScanResult {
        equation,
        new_quantities,
    })
}

fn readable_log_line(line: &str) -> Option<String> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
        return nonempty(line.to_string());
    };
    if value.get("type").and_then(|value| value.as_str()) == Some("assistant") {
        let message = value.get("message").unwrap_or(&value);
        let mut parts = Vec::new();
        if let Some(content) = message.get("content").and_then(|value| value.as_array()) {
            for item in content {
                if let Some(text) = item.get("text").and_then(|value| value.as_str()) {
                    parts.push(text.to_string());
                } else if item.get("type").and_then(|value| value.as_str()) == Some("tool_use")
                    && let Some(name) = item.get("name").and_then(|value| value.as_str())
                {
                    parts.push(format!("-> {name}"));
                }
            }
        }
        return nonempty(parts.join(" "));
    }
    if let Some(msg) = value.get("msg") {
        let mut parts = Vec::new();
        if let Some(kind) = msg.get("type").and_then(|value| value.as_str()) {
            parts.push(kind.to_string());
        }
        find_text_fields(msg, &mut parts);
        return nonempty(parts.join(": "));
    }
    Some(line.to_string())
}

fn find_text_fields(value: &serde_json::Value, out: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, value) in map {
                if key == "text"
                    && let Some(text) = value.as_str()
                {
                    out.push(text.to_string());
                } else {
                    find_text_fields(value, out);
                }
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                find_text_fields(value, out);
            }
        }
        _ => {}
    }
}

fn nonempty(value: String) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use nullspace_core::Variable;

    #[test]
    fn parse_scan_result_rejects_empty_result() {
        let ids = HashSet::new();
        assert!(parse_scan_result(r#"{"quantities":[],"equations":[]}"#, &ids).is_err());
    }

    #[test]
    fn parse_scan_result_splits_new_vs_existing_quantities() {
        let existing = Quantity::new("E".to_string());
        let fresh = Quantity::new("m".to_string());
        let mut equation = Equation::new("Energy".to_string(), "E=mc^2".to_string());
        equation.variables = vec![Variable {
            symbol: "E".to_string(),
            description: "energy".to_string(),
            quantity_id: Some(existing.id),
        }];
        let raw = serde_json::to_string(&json!({
            "quantities": [existing, fresh],
            "equations": [equation],
        }))
        .unwrap();
        let existing_ids = [existing.id].into_iter().collect();

        let result = parse_scan_result(&raw, &existing_ids).unwrap();

        assert_eq!(result.new_quantities.len(), 1);
        assert_eq!(result.new_quantities[0].symbol, "m");
    }

    #[test]
    fn parse_scan_result_nulls_dangling_quantity_id() {
        let unknown = QuantityId::new();
        let mut equation = Equation::new("Energy".to_string(), "E=mc^2".to_string());
        equation.variables = vec![Variable {
            symbol: "E".to_string(),
            description: "energy".to_string(),
            quantity_id: Some(unknown),
        }];
        let raw = serde_json::to_string(&json!({
            "quantities": [],
            "equations": [equation],
        }))
        .unwrap();

        let result = parse_scan_result(&raw, &HashSet::new()).unwrap();

        assert_eq!(result.equation.variables[0].quantity_id, None);
    }

    #[test]
    fn build_prompt_embeds_schema_and_quantities() {
        let quantity = Quantity::new("E".to_string());
        let schema = serde_json::to_string(&crate::export_file_schema()).unwrap();

        let prompt = build_prompt("capture.png", &schema, &[quantity], ScanAgent::Codex);

        assert!(prompt.contains("capture.png"));
        assert!(prompt.contains("properties"));
        assert!(prompt.contains("\"E\""));
    }
}
