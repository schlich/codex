//! Presentation adapters for execution-style MCP tools.
//!
//! Adapters interpret only documented fields. The MCP cell separately retains the received
//! result for its transcript: this projection must never become the source of raw output.

use super::McpInvocation;
use super::McpToolCallCell;
use crate::motion::MotionMode;
use crate::motion::ReducedMotionIndicator;
use crate::motion::activity_indicator;
use crate::render::highlight::highlight_code_to_lines;
use crate::style::StatusTone;
use crate::style::accent_style;
use crate::style::status_style;
use crate::terminal_hyperlinks::HyperlinkLine;
use crate::terminal_hyperlinks::prefix_hyperlink_lines;
use crate::terminal_hyperlinks::remap_source_wrapped_line;
use crate::tool_output::ToolOutputPreview;
use crate::wrapping::RtOptions;
use crate::wrapping::word_wrap_line_with_source;
use codex_protocol::mcp::CallToolResult;
use ratatui::style::Style;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;
use serde_json::Value;
use textwrap::WordSplitter;

/// Recognition and wire-format interpretation belong here, not in the shared renderer.
#[derive(Clone, Copy)]
pub(super) enum ExecutionTool {
    Nushell,
}

impl ExecutionTool {
    pub(super) fn detect(invocation: &McpInvocation) -> Option<Self> {
        match (invocation.server.as_str(), invocation.tool.as_str()) {
            ("nushell", "evaluate") if invocation.arguments.as_ref()?.get("input")?.is_string() => {
                Some(Self::Nushell)
            }
            _ => None,
        }
    }

    fn source<'a>(self, invocation: &'a McpInvocation) -> &'a str {
        match self {
            Self::Nushell => invocation
                .arguments
                .as_ref()
                .and_then(|args| args["input"].as_str()),
        }
        .unwrap_or_default()
    }

    fn label(self) -> &'static str {
        match self {
            Self::Nushell => "Nushell",
        }
    }

    fn language(self) -> &'static str {
        match self {
            Self::Nushell => "nu",
        }
    }

    pub(super) fn result(self, result: &CallToolResult) -> ExecutionResult {
        match self {
            Self::Nushell => ExecutionResult::nushell(result),
        }
    }
}

#[derive(Debug, Default)]
pub(super) struct ExecutionResult {
    cwd_after: Option<String>,
    exit_code: Option<i64>,
    /// None means the adapter could not interpret the payload; show the original content.
    output: Option<Vec<ExecutionOutput>>,
}

#[derive(Debug)]
struct ExecutionOutput {
    text: String,
    diagnostic: bool,
}

impl ExecutionResult {
    pub(super) fn failed(&self) -> bool {
        self.exit_code.is_some_and(|code| code != 0)
    }

    fn nushell(result: &CallToolResult) -> Self {
        let Some(data) = result
            .structured_content
            .as_ref()
            .and_then(Value::as_object)
        else {
            return Self::default();
        };
        let mut presentation = Self::default();
        let mut output = Vec::new();
        if result.is_error == Some(true) {
            let Some(message) = data.get("msg").and_then(Value::as_str) else {
                return presentation;
            };
            output.push(ExecutionOutput {
                text: message.to_owned(),
                diagnostic: true,
            });
            if let Some(help) = data.get("help").and_then(Value::as_str) {
                output.push(ExecutionOutput {
                    text: format!("Help: {help}"),
                    diagnostic: true,
                });
            }
        } else {
            // cwd is the persistent shell's directory *after* evaluation, not a per-call input.
            presentation.cwd_after = data
                .get("cwd")
                .and_then(Value::as_str)
                .filter(|cwd| !cwd.is_empty() && *cwd != "unknown")
                .map(str::to_owned);
            let note = data.get("note").and_then(Value::as_str);
            if let Some(value) = data.get("output") {
                // `complete` is the only observed output shape carrying a process exit status.
                // Never infer it from the MCP success flag or a nested application record.
                let complete = value.as_object().filter(|record| {
                    record.len() == 3
                        && record.get("stdout").is_some_and(Value::is_string)
                        && record.get("stderr").is_some_and(Value::is_string)
                        && record.get("exit_code").and_then(Value::as_i64).is_some()
                });
                if let Some(record) = complete {
                    presentation.exit_code = record["exit_code"].as_i64();
                    output.push(ExecutionOutput {
                        text: record["stderr"].as_str().unwrap_or_default().to_owned(),
                        diagnostic: true,
                    });
                    output.push(ExecutionOutput {
                        text: record["stdout"].as_str().unwrap_or_default().to_owned(),
                        diagnostic: false,
                    });
                } else if !value.is_null() {
                    output.push(ExecutionOutput {
                        text: value
                            .as_str()
                            .map(str::to_owned)
                            .unwrap_or_else(|| value.to_string()),
                        diagnostic: false,
                    });
                }
            } else if note.is_none() {
                return presentation;
            }
            if let Some(note) = note {
                // An inconsistent envelope may contain both fields. Never let its note hide
                // a real diagnostic under the preview's row budget.
                output.push(ExecutionOutput {
                    text: note.to_owned(),
                    diagnostic: false,
                });
            }
        }
        presentation.output = Some(output);
        presentation
    }
}

#[cfg(test)]
#[path = "mcp_execution_tests.rs"]
mod tests;

fn wrap(lines: Vec<Line<'static>>, width: usize) -> Vec<HyperlinkLine> {
    let options = RtOptions::new(width.max(1)).word_splitter(WordSplitter::NoHyphenation);
    lines
        .into_iter()
        .flat_map(|line| {
            let line = HyperlinkLine::new(line);
            remap_source_wrapped_line(
                &line,
                word_wrap_line_with_source(&line.line, options.clone()),
            )
        })
        .collect()
}

impl McpToolCallCell {
    pub(super) fn execution_lines(&self, width: u16) -> Option<Vec<HyperlinkLine>> {
        let tool = ExecutionTool::detect(&self.invocation)?;
        let result = self.execution_result.as_ref();
        let exit = result.and_then(|result| result.exit_code);
        let failed = self.success() == Some(false) || exit.is_some_and(|code| code != 0);
        let (label, style) = if self.result.is_none() {
            ("Running", accent_style())
        } else if failed {
            ("Failed", status_style(StatusTone::Failure))
        } else if exit == Some(0) {
            ("Succeeded", status_style(StatusTone::Success))
        } else {
            ("Completed", status_style(StatusTone::Success))
        };
        let marker = if self.result.is_none() {
            activity_indicator(
                Some(self.start_time),
                MotionMode::from_animations_enabled(self.animations_enabled),
                ReducedMotionIndicator::StaticBullet,
            )
            .unwrap_or_else(|| Span::styled("•", style))
        } else {
            Span::styled("•", style)
        };
        let mut header = vec![
            marker,
            " ".into(),
            Span::styled(format!("{} · {label}", tool.label()), style),
        ];
        // Frozen snapshots and completed history must not keep accumulating elapsed time.
        let duration = self.duration.or_else(|| {
            (self.result.is_none() && self.animations_enabled).then(|| self.start_time.elapsed())
        });
        if let Some(exit) = exit {
            header.push(Span::styled(format!(" · exit {exit}"), style));
        }
        if let Some(duration) = duration {
            header.push(format!(" · {:.1}s", duration.as_secs_f64()).dim());
        }
        let indent = usize::from(width).saturating_sub(1).min(2);
        let body_width = usize::from(width).saturating_sub(indent).max(1);
        let prefix = " ".repeat(indent);
        let header = HyperlinkLine::new(Line::from(header));
        let header_options = RtOptions::new(usize::from(width).max(1))
            .subsequent_indent(prefix.clone().into())
            .word_splitter(WordSplitter::NoHyphenation);
        let mut lines = remap_source_wrapped_line(
            &header,
            word_wrap_line_with_source(&header.line, header_options),
        );
        let source = wrap(
            highlight_code_to_lines(tool.source(&self.invocation), tool.language()),
            body_width,
        );
        lines.extend(prefix_hyperlink_lines(
            source,
            prefix.clone().into(),
            prefix.clone().into(),
        ));
        if let Some(cwd) = result.and_then(|result| result.cwd_after.as_deref()) {
            let cwd = wrap(
                vec![Line::from(format!("cwd after: {cwd}").dim())],
                body_width,
            );
            lines.extend(prefix_hyperlink_lines(
                cwd,
                prefix.clone().into(),
                prefix.clone().into(),
            ));
        }
        let mut preview = ToolOutputPreview::new(body_width, 0);
        match &self.result {
            Some(Ok(raw)) => {
                if let Some(output) = result.and_then(|result| result.output.as_ref()) {
                    for block in output {
                        let style = if block.diagnostic || failed {
                            status_style(StatusTone::Failure)
                        } else {
                            Style::default().dim()
                        };
                        for line in block.text.lines() {
                            preview.push_line(Line::styled(line, style));
                        }
                    }
                } else {
                    for block in &raw.content {
                        let text = block.render();
                        for line in text.lines() {
                            preview.push_line(Line::styled(
                                line,
                                if failed {
                                    status_style(StatusTone::Failure)
                                } else {
                                    Style::default().dim()
                                },
                            ));
                        }
                    }
                }
            }
            Some(Err(error)) => {
                for line in error.lines() {
                    preview.push_line(Line::styled(line, status_style(StatusTone::Failure)));
                }
            }
            None => {}
        }
        lines.extend(prefix_hyperlink_lines(
            preview.finish_hyperlink_lines(),
            prefix.clone().into(),
            prefix.into(),
        ));
        Some(lines)
    }

    /// Raw protocol fields are displayed only in the existing transcript for supported tools.
    pub(super) fn execution_metadata_lines(&self) -> Vec<Line<'static>> {
        if ExecutionTool::detect(&self.invocation).is_none() {
            return Vec::new();
        }
        let Some(Ok(result)) = &self.result else {
            return Vec::new();
        };
        let mut lines = Vec::new();
        if let Some(raw) = &self.execution_raw_result {
            // Include the original wire blocks, including annotations, unknown fields, and
            // escaped line endings that the human-readable transcript cannot represent exactly.
            lines.push(Line::from(format!(
                "raw result: {}",
                serde_json::json!(raw)
            )));
        }
        if let Some(value) = &result.structured_content {
            lines.push(Line::from(format!("structured result: {value}")));
        }
        if let Some(value) = &result.meta {
            lines.push(Line::from(format!("MCP metadata: {value}")));
        }
        lines
    }
}
