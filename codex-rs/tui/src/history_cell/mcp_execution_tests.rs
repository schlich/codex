use super::*;
use crate::history_cell::HistoryCell;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::widgets::Paragraph;
use serde_json::json;
use std::time::Duration;

fn pending(source: &str) -> McpToolCallCell {
    McpToolCallCell::new(
        "execution".into(),
        McpInvocation {
            server: "nushell".into(),
            tool: "evaluate".into(),
            arguments: Some(json!({"input": source})),
        },
        false,
    )
}

fn completed(source: &str, data: Option<Value>, error: bool) -> McpToolCallCell {
    let mut cell = pending(source);
    cell.complete(
        Duration::from_millis(1250),
        Ok(CallToolResult {
            // The JSON mirror is interpreted; the actual received NUON remains unmodified.
            content: vec![
                json!({"type": "text", "text": "{cwd:/workspace,history_index:7,output:raw}"}),
            ],
            structured_content: data,
            is_error: Some(error),
            meta: None,
        }),
    );
    cell
}

fn text(lines: Vec<Line<'static>>) -> String {
    lines
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

fn captured(fixture: &str) -> McpToolCallCell {
    #[derive(serde::Deserialize)]
    struct Captured {
        input: String,
        result: CallToolResult,
    }
    let fixture: Captured = serde_json::from_str(fixture).unwrap();
    let mut cell = pending(&fixture.input);
    cell.complete(Duration::from_millis(50), Ok(fixture.result));
    cell
}

const CAPTURED_SUCCESS: &str = include_str!("../../tests/fixtures/nushell-evaluate-success.json");
const CAPTURED_FAILURE: &str = include_str!("../../tests/fixtures/nushell-evaluate-failure.json");

fn screen(cell: &McpToolCallCell, width: u16) -> String {
    let lines = cell.display_lines(width);
    let height = u16::try_from(lines.len()).unwrap();
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|frame| frame.render_widget(Paragraph::new(lines), frame.area()))
        .unwrap();
    let buffer = terminal.backend().buffer();
    (0..height)
        .map(|y| {
            let mut row = String::new();
            let mut x = 0;
            while x < width {
                let symbol = buffer[(x, y)].symbol();
                row.push_str(symbol);
                x += u16::try_from(crate::width::display_width(symbol).max(1)).unwrap();
            }
            row.trim_end().to_owned()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn demonstrations() -> String {
    let source = "# Inspect generated files\nlet names = [alpha beta]\n$names | each {|name|\n    $\"file: ($name)\"\n}";
    let mut transport_failure = pending("open missing.nu");
    transport_failure.complete(
        Duration::from_millis(20),
        Err("MCP connection closed".into()),
    );
    let examples = [
        ("Captured Nushell success", captured(CAPTURED_SUCCESS)),
        ("Captured Nushell failure", captured(CAPTURED_FAILURE)),
        ("Running", pending(source)),
        (
            "Empty evaluation",
            completed(
                "let x = 42",
                Some(json!({"cwd":"/workspace", "output":null})),
                false,
            ),
        ),
        (
            "Exit unknown",
            completed(
                source,
                Some(json!({"cwd":"/workspace", "output":"file: alpha\nfile: beta"})),
                false,
            ),
        ),
        (
            "Explicit success",
            completed(
                "^printf 'done' | complete",
                Some(
                    json!({"cwd":"/workspace", "output":{"stdout":"done\n", "stderr":"", "exit_code":0}}),
                ),
                false,
            ),
        ),
        (
            "Explicit failure",
            completed(
                "^tool check | complete",
                Some(
                    json!({"cwd":"/workspace", "output":{"stdout":"one\ntwo\nthree\nfour", "stderr":"permission denied\ncheck credentials", "exit_code":7}}),
                ),
                false,
            ),
        ),
        (
            "Evaluation error",
            completed(
                "open missing.nu",
                Some(
                    json!({"msg":"File not found", "help":"Check the supplied path", "labels":[{"line":1,"column":6}]}),
                ),
                true,
            ),
        ),
        (
            "Truncated at server",
            completed(
                "$history.7",
                Some(
                    json!({"cwd":"/workspace", "history_index":8, "note":"output truncated, full result in $history.8"}),
                ),
                false,
            ),
        ),
        ("Transport failure", transport_failure),
        ("Legacy response", completed("1 + 1", None, false)),
        (
            "Long Unicode command",
            pending(
                "let description = 'long command with 页面 and 👩‍💻 plus https://example.com/a/very/long/path/without/breaks'; $description",
            ),
        ),
    ];
    [80, 32]
        .into_iter()
        .flat_map(|width| {
            examples.iter().map(move |(name, cell)| {
                format!("{name} ({width} columns)\n{}", screen(cell, width))
            })
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[test]
fn execution_tool_states_and_wrapping() {
    insta::assert_snapshot!("execution_tool_states", demonstrations());
}

#[test]
fn execution_captured_responses_preserve_raw_payloads() {
    for fixture in [CAPTURED_SUCCESS, CAPTURED_FAILURE] {
        let data: Value = serde_json::from_str(fixture).unwrap();
        let cell = captured(fixture);
        let raw = text(cell.raw_lines());
        assert!(raw.contains(&data["result"]["content"].to_string()));
        assert!(raw.contains(&data["result"]["structuredContent"].to_string()));
        let code = data["result"]["structuredContent"]["output"]["exit_code"]
            .as_i64()
            .unwrap();
        assert!(text(cell.display_lines(120)).contains(&format!("exit {code}")));
    }
}

/// A deterministic working demonstration through the real renderer and terminal backend.
#[test]
#[ignore = "run explicitly with --ignored --nocapture to print the demonstration"]
fn execution_tool_demo() {
    println!("{}", demonstrations());
}

#[test]
fn execution_recognition_requires_exact_tool_and_source() {
    for (server, tool, args, supported) in [
        ("nushell", "evaluate", json!({"input":""}), true),
        ("nushell", "evaluate", json!({"input":"1 + 1"}), true),
        ("nushell", "evaluate", json!({"input":4}), false),
        ("nushell", "evaluate", json!({"command":"1 + 1"}), false),
        ("nushell", "evaluate", Value::Null, false),
        ("other", "evaluate", json!({"input":"1 + 1"}), false),
        ("nushell", "list_commands", json!({"input":"1 + 1"}), false),
    ] {
        let invocation = McpInvocation {
            server: server.into(),
            tool: tool.into(),
            arguments: Some(args),
        };
        assert_eq!(ExecutionTool::detect(&invocation).is_some(), supported);
    }
}

#[test]
fn only_complete_records_supply_exit_status() {
    for output in [
        json!(null),
        json!("exit_code: 9"),
        json!({"exit_code":9}),
        json!({"stdout":"", "stderr":"", "exit_code":"9"}),
        json!({"stdout":"", "stderr":null, "exit_code":9}),
        json!({"nested":{"stdout":"", "stderr":"", "exit_code":9}}),
        json!({"stdout":"", "stderr":"", "exit_code":9, "application":"data"}),
    ] {
        let cell = completed("do something", Some(json!({"output": output})), false);
        assert_eq!(cell.execution_result.as_ref().unwrap().exit_code, None);
        assert!(text(cell.display_lines(120)).starts_with("• Nushell · Completed · 1.2s"));
    }
    for code in [i64::MIN, -1, 0, 1, i64::MAX] {
        let cell = completed(
            "^tool | complete",
            Some(json!({"output":{"stdout":"", "stderr":"", "exit_code":code}})),
            false,
        );
        let shown = text(cell.display_lines(120));
        assert!(shown.contains(&format!("exit {code}")));
        assert!(shown.contains(if code == 0 { "Succeeded" } else { "Failed" }));
    }
}

#[test]
fn malformed_metadata_falls_back_without_inventing_fields() {
    for data in [
        None,
        Some(Value::Null),
        Some(json!([])),
        Some(json!({"cwd":42})),
        Some(json!({"note":9})),
        Some(json!({"msg":"not an error"})),
    ] {
        let cell = completed("1 + 1", data, false);
        let shown = text(cell.display_lines(120));
        assert!(shown.contains("history_index:7,output:raw"));
        assert!(!shown.contains("exit "));
        assert!(!shown.contains("cwd after:"));
    }
    for cwd in [json!(null), json!(""), json!("unknown"), json!(42)] {
        let cell = completed("1", Some(json!({"cwd":cwd, "output":1})), false);
        assert!(!text(cell.display_lines(120)).contains("cwd after:"));
    }
}

#[test]
fn execution_errors_take_priority_over_stdout() {
    let cell = completed(
        "^tool | complete",
        Some(
            json!({"output":{"stdout":"one\ntwo\nthree\nfour\nfive", "stderr":"important error", "exit_code":1}}),
        ),
        false,
    );
    let display = text(cell.display_lines(80));
    assert!(display.find("important error").unwrap() < display.find("one").unwrap());
    assert!(display.contains("+3 lines"));
    let diagnostic = cell
        .display_lines(80)
        .into_iter()
        .find(|line| line.to_string().contains("important error"))
        .unwrap();
    assert!(
        diagnostic
            .spans
            .iter()
            .any(|span| span.style == status_style(StatusTone::Failure))
    );
}

#[test]
fn execution_inconsistent_note_cannot_hide_diagnostics() {
    let cell = completed(
        "^tool | complete",
        Some(
            json!({"note":"note ".repeat(100), "output":{"stdout":"", "stderr":"critical error", "exit_code":1}}),
        ),
        false,
    );
    assert!(text(cell.display_lines(32)).contains("critical error"));
}

#[test]
fn execution_large_output_stays_bounded_without_losing_the_tail() {
    let stdout = format!("{}\nretained tail", "页面".repeat(100_000));
    let cell = completed(
        "^tool | complete",
        Some(json!({"output":{
            "stdout":stdout, "stderr":"critical error", "exit_code":1
        }})),
        false,
    );
    let lines = cell.display_lines(80);
    assert!(text(lines.clone()).contains("critical error"));
    // One header, one source line, three output rows, and the omission hint.
    assert_eq!(lines.len(), 6);
    assert!(
        lines
            .last()
            .unwrap()
            .to_string()
            .contains("to view transcript")
    );
    assert!(text(cell.raw_lines()).contains("retained tail"));
}

#[test]
fn execution_transcript_retains_arguments_streams_and_metadata() {
    let source = "# Read results\n  ^tool | complete\n";
    let structured = json!({"cwd":"/after", "history_index":7, "timestamp":"2026-10-17T00:00:00Z", "output":{"stdout":"all\nstdout\n", "stderr":"all\nstderr\n", "exit_code":2}, "future_field":{"extra":true}});
    let mut cell = pending(source);
    let raw = "{output:{stdout:'all stdout',stderr:'all stderr',exit_code:2}}\nraw tail";
    cell.complete(
        Duration::ZERO,
        Ok(CallToolResult {
            content: vec![json!({"type":"text", "text":raw})],
            structured_content: Some(structured.clone()),
            is_error: Some(false),
            meta: Some(json!({"trace":"keep-me"})),
        }),
    );
    let transcript = text(cell.transcript_lines(4000));
    let raw_lines = text(cell.raw_lines());
    for view in [&transcript, &raw_lines] {
        for line in raw.lines() {
            assert!(view.contains(line));
        }
        assert!(view.contains(&structured.to_string()));
        assert!(view.contains("MCP metadata: {\"trace\":\"keep-me\"}"));
        assert!(view.contains(&serde_json::to_string(source).unwrap()));
    }
    assert!(!text(cell.display_lines(80)).contains("future_field"));
}

#[test]
fn execution_source_keeps_indentation_and_logical_ranges() {
    // Check a family of widths and Unicode sources: wrapping may change, source must not.
    for source in [
        "",
        "a\n\n    b",
        "let x = '页面 👩‍💻 e\u{301}'",
        "    abcdefghijklmnopqrstuvwxyz",
        "https://example.com/abcdefghijklmnopqrstuvwxyz",
    ] {
        let cell = pending(source);
        for width in 8..100 {
            let lines = cell.execution_lines(width).unwrap();
            for line in &lines {
                assert!(
                    line.line.width() <= usize::from(width),
                    "width {width}: {:?}",
                    line.line
                );
            }
            // Every source row has provenance pointing to the complete original logical line.
            for logical in source.lines().filter(|line| !line.is_empty()) {
                assert!(
                    lines.iter().any(|line| line
                        .source
                        .as_ref()
                        .is_some_and(|s| s.text.as_ref() == logical)),
                    "missing source: {logical:?}"
                );
            }
        }
    }
    assert!(text(pending("a\n\n    b").display_lines(80)).contains("  a\n  \n      b"));
}

#[test]
fn execution_uses_shared_syntax_theme_or_plain_fallback() {
    let source = "let x = 42";
    let original = crate::render::highlight::current_syntax_theme();
    for color in [
        syntect::highlighting::Color::WHITE,
        syntect::highlighting::Color::BLACK,
    ] {
        let mut theme = syntect::highlighting::Theme::default();
        theme.settings.foreground = Some(color);
        crate::render::highlight::set_syntax_theme(theme);
        let highlighted = highlight_code_to_lines(source, "nu");
        let shown = pending(source).display_lines(80);
        assert_eq!(
            shown[1].spans.iter().skip(1).cloned().collect::<Vec<_>>(),
            highlighted[0].spans
        );
    }
    crate::render::highlight::set_syntax_theme(original);
}

#[test]
fn execution_mcp_failure_is_never_overridden_by_zero_exit() {
    let cell = completed(
        "^tool | complete",
        Some(json!({"output":{"stdout":"", "stderr":"", "exit_code":0}})),
        true,
    );
    assert!(text(cell.display_lines(120)).contains("Failed"));
}

#[test]
fn execution_compact_and_display_agree() {
    let cell = completed("1 + 1", Some(json!({"output":2})), false);
    for width in [16, 32, 80, 120] {
        assert_eq!(
            cell.compact_hyperlink_lines(width),
            cell.display_hyperlink_lines(width)
        );
    }
}

#[test]
fn execution_narrow_header_keeps_exit_status_with_its_label() {
    let cell = captured(CAPTURED_FAILURE);
    let lines = cell.display_lines(32);
    assert!(lines[0].to_string().contains("exit 1"));
    assert!(lines[1].to_string().starts_with("  "));
}
