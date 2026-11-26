//! Provider end-to-end tests against a tiny local HTTP server serving canned SSE streams.

use ruck_harness_llm::anthropic::Anthropic;
use ruck_harness_llm::openai::OpenAi;
use ruck_harness_llm::{
    ContentBlock, Message, Provider, Request, StopReason, StreamEvent, ToolSpec, Usage,
};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

struct Captured {
    headers: Vec<(String, String)>,
    body: Value,
    path: String,
}

impl Captured {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// Serve exactly one request with the given status/content-type/body, then close.
/// Returns the base URL and a handle yielding the captured request.
async fn serve(
    status: &str,
    content_type: &'static str,
    body: String,
) -> (String, tokio::task::JoinHandle<Captured>) {
    // Loopback must never go through the sandbox HTTPS/HTTP proxy.
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        for k in ["NO_PROXY", "no_proxy"] {
            // Safe: runs once before any client in this process is built.
            std::env::set_var(k, "127.0.0.1,localhost");
        }
    });
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let status = status.to_string();
    let h = tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut buf = Vec::new();
        let mut tmp = [0u8; 4096];
        let (head_end, content_len) = loop {
            let n = sock.read(&mut tmp).await.unwrap();
            assert!(n > 0, "client closed early");
            buf.extend_from_slice(&tmp[..n]);
            if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                let head = String::from_utf8_lossy(&buf[..p]).to_ascii_lowercase();
                let cl = head
                    .lines()
                    .find_map(|l| l.strip_prefix("content-length:"))
                    .map(|v| v.trim().parse::<usize>().unwrap())
                    .unwrap_or(0);
                break (p + 4, cl);
            }
        };
        while buf.len() < head_end + content_len {
            let n = sock.read(&mut tmp).await.unwrap();
            assert!(n > 0);
            buf.extend_from_slice(&tmp[..n]);
        }
        let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
        let mut lines = head.lines();
        let path = lines
            .next()
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap()
            .to_string();
        let headers = lines
            .filter_map(|l| l.split_once(':'))
            .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
            .collect();
        let body_json =
            serde_json::from_slice(&buf[head_end..head_end + content_len]).unwrap_or(Value::Null);

        // Connection-close framing; write the body in small pieces to exercise chunk splitting.
        let resp_head = format!(
            "HTTP/1.1 {status}\r\ncontent-type: {content_type}\r\ncache-control: no-cache\r\nconnection: close\r\n\r\n"
        );
        sock.write_all(resp_head.as_bytes()).await.unwrap();
        for piece in body.as_bytes().chunks(37) {
            sock.write_all(piece).await.unwrap();
            sock.flush().await.unwrap();
            tokio::task::yield_now().await;
        }
        let _ = sock.shutdown().await;
        Captured {
            headers,
            body: body_json,
            path,
        }
    });
    (url, h)
}

fn sse(events: &[(&str, Value)]) -> String {
    events
        .iter()
        .map(|(name, data)| {
            if name.is_empty() {
                format!("data: {data}\n\n")
            } else {
                format!("event: {name}\ndata: {data}\n\n")
            }
        })
        .collect()
}

fn request() -> Request {
    Request {
        model: "test-model".into(),
        system: "be helpful".into(),
        messages: vec![Message::user_text("list files")],
        tools: vec![ToolSpec {
            name: "Bash".into(),
            description: "run a command".into(),
            input_schema: json!({"type":"object","properties":{"command":{"type":"string"}}}),
        }],
        max_tokens: 1024,
        thinking: None,
        temperature: None,
        top_p: None,
    }
}

fn drain(mut rx: tokio::sync::mpsc::UnboundedReceiver<StreamEvent>) -> Vec<StreamEvent> {
    let mut v = vec![];
    while let Ok(e) = rx.try_recv() {
        v.push(e);
    }
    v
}

#[tokio::test]
async fn anthropic_stream_thinking_text_and_tool() {
    let body = sse(&[
        (
            "message_start",
            json!({"type":"message_start","message":{"id":"msg_1","role":"assistant","usage":{"input_tokens":25,"cache_read_input_tokens":5,"output_tokens":1}}}),
        ),
        ("ping", json!({"type":"ping"})),
        (
            "content_block_start",
            json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}),
        ),
        (
            "content_block_delta",
            json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"Let me "}}),
        ),
        (
            "content_block_delta",
            json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"think."}}),
        ),
        (
            "content_block_delta",
            json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"EqQB"}}),
        ),
        (
            "content_block_delta",
            json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"Cg=="}}),
        ),
        (
            "content_block_stop",
            json!({"type":"content_block_stop","index":0}),
        ),
        (
            "content_block_start",
            json!({"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}),
        ),
        (
            "content_block_delta",
            json!({"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"I'll run "}}),
        ),
        (
            "content_block_delta",
            json!({"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"ls."}}),
        ),
        (
            "content_block_stop",
            json!({"type":"content_block_stop","index":1}),
        ),
        (
            "content_block_start",
            json!({"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"toolu_01","name":"Bash","input":{}}}),
        ),
        (
            "content_block_delta",
            json!({"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":""}}),
        ),
        (
            "content_block_delta",
            json!({"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"{\"comm"}}),
        ),
        (
            "content_block_delta",
            json!({"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"and\": \"ls -"}}),
        ),
        (
            "content_block_delta",
            json!({"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"la\"}"}}),
        ),
        (
            "content_block_stop",
            json!({"type":"content_block_stop","index":2}),
        ),
        (
            "message_delta",
            json!({"type":"message_delta","delta":{"stop_reason":"tool_use","stop_sequence":null},"usage":{"output_tokens":77}}),
        ),
        ("message_stop", json!({"type":"message_stop"})),
    ]);
    let (url, cap) = serve("200 OK", "text/event-stream", body).await;
    let p = Anthropic::new("sk-ant-test", Some(url));
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut req = request();
    req.thinking = Some(json!({"type":"enabled","budget_tokens":2000}));
    let r = p.complete(req, tx).await.unwrap();

    assert_eq!(r.stop_reason, StopReason::ToolUse);
    assert_eq!(
        r.usage,
        Usage {
            input_tokens: 30,
            output_tokens: 77,
            cache_read_tokens: Some(5),
            cache_write_tokens: None,
        }
    );
    assert_eq!(
        r.message.content,
        vec![
            ContentBlock::Thinking {
                thinking: "Let me think.".into(),
                signature: Some("EqQBCg==".into())
            },
            ContentBlock::Text {
                text: "I'll run ls.".into()
            },
            ContentBlock::ToolUse {
                id: "toolu_01".into(),
                name: "Bash".into(),
                input: json!({"command":"ls -la"})
            },
        ]
    );
    assert_eq!(
        drain(rx),
        vec![
            StreamEvent::ThinkingDelta("Let me ".into()),
            StreamEvent::ThinkingDelta("think.".into()),
            StreamEvent::TextDelta("I'll run ".into()),
            StreamEvent::TextDelta("ls.".into()),
            StreamEvent::ToolUseStart {
                id: "toolu_01".into(),
                name: "Bash".into()
            },
        ]
    );

    let c = cap.await.unwrap();
    assert_eq!(c.path, "/v1/messages");
    assert_eq!(c.header("x-api-key"), Some("sk-ant-test"));
    assert_eq!(c.header("anthropic-version"), Some("2023-06-01"));
    assert!(c.header("authorization").is_none());
    assert_eq!(c.body["stream"], true);
    assert_eq!(c.body["model"], "test-model");
    assert_eq!(c.body["max_tokens"], 1024);
    assert_eq!(c.body["thinking"]["budget_tokens"], 2000);
    assert_eq!(c.body["system"][0]["text"], "be helpful");
    assert_eq!(c.body["tools"][0]["name"], "Bash");
    assert_eq!(c.body["tools"][0]["input_schema"]["type"], "object");
    assert_eq!(c.body["messages"][0]["content"][0]["text"], "list files");
}

#[tokio::test]
async fn anthropic_error_surfaces_body() {
    let body =
        r#"{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}"#;
    let (url, _cap) = serve("401 Unauthorized", "application/json", body.into()).await;
    let p = Anthropic::new("bad", Some(url));
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let e = p.complete(request(), tx).await.unwrap_err().to_string();
    assert!(e.contains("401"), "{e}");
    assert!(e.contains("invalid x-api-key"), "{e}");
}

#[tokio::test]
async fn anthropic_stream_error_event() {
    let body = sse(&[(
        "error",
        json!({"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}),
    )]);
    let (url, _cap) = serve("200 OK", "text/event-stream", body).await;
    let p = Anthropic::new("k", Some(url));
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let e = p.complete(request(), tx).await.unwrap_err().to_string();
    assert!(e.contains("Overloaded"), "{e}");
}

/// Chunk shapes recorded from Ollama 0.40 (`/v1/chat/completions`, gemma4:26b, streaming):
/// `reasoning` deltas, then each tool call complete in one chunk (with an empty `content`),
/// every call at `index` 0, then an empty delta carrying `finish_reason`.
#[tokio::test]
async fn openai_stream_ollama_complete_tool_calls_after_reasoning() {
    let chunk = |delta: Value, fr: Value| json!({"id":"chatcmpl-174","object":"chat.completion.chunk","system_fingerprint":"fp_ollama","model":"gemma4:26b","choices":[{"index":0,"delta":delta,"finish_reason":fr}]});
    let mut body = sse(&[
        (
            "",
            chunk(
                json!({"role":"assistant","content":"","reasoning":"The"}),
                Value::Null,
            ),
        ),
        (
            "",
            chunk(json!({"content":"","reasoning":" user"}), Value::Null),
        ),
        (
            "",
            chunk(
                json!({"content":"","tool_calls":[{"id":"call_ky12ndoi","index":0,"type":"function","function":{"name":"TodoWrite","arguments":"{\"todos\":[{\"content\":\"look\",\"status\":\"in_progress\"}]}"}}]}),
                Value::Null,
            ),
        ),
        (
            "",
            chunk(
                json!({"tool_calls":[{"id":"call_zz","index":0,"type":"function","function":{"name":"Bash","arguments":"{\"command\":\"ls\"}"}}]}),
                Value::Null,
            ),
        ),
        ("", chunk(json!({}), json!("tool_calls"))),
    ]);
    body.push_str("data: [DONE]\n\n");
    let (url, _cap) = serve("200 OK", "text/event-stream", body).await;
    let p = OpenAi::new(None, Some(format!("{url}/v1")));
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let r = p.complete(request(), tx).await.unwrap();
    assert_eq!(r.stop_reason, StopReason::ToolUse);
    assert_eq!(
        r.message.content,
        vec![
            ContentBlock::Thinking {
                thinking: "The user".into(),
                signature: None
            },
            ContentBlock::ToolUse {
                id: "call_ky12ndoi".into(),
                name: "TodoWrite".into(),
                input: json!({"todos":[{"content":"look","status":"in_progress"}]})
            },
            ContentBlock::ToolUse {
                id: "call_zz".into(),
                name: "Bash".into(),
                input: json!({"command":"ls"})
            },
        ]
    );
}

/// The failure mode behind the report: the model finishes its reasoning and Ollama closes the
/// stream with `finish_reason: "stop"`, no content and no tool call. That is a real end of turn,
/// not something the parser dropped.
#[tokio::test]
async fn openai_stream_reasoning_only_stop_is_end_turn() {
    let chunk = |delta: Value, fr: Value| json!({"id":"c","object":"chat.completion.chunk","model":"m","choices":[{"index":0,"delta":delta,"finish_reason":fr}]});
    let mut body = sse(&[
        (
            "",
            chunk(
                json!({"role":"assistant","content":"","reasoning":"Let's start."}),
                Value::Null,
            ),
        ),
        ("", chunk(json!({}), json!("stop"))),
    ]);
    body.push_str("data: [DONE]\n\n");
    let (url, _cap) = serve("200 OK", "text/event-stream", body).await;
    let p = OpenAi::new(None, Some(format!("{url}/v1")));
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let r = p.complete(request(), tx).await.unwrap();
    assert_eq!(r.stop_reason, StopReason::EndTurn);
    assert_eq!(
        r.message.content,
        vec![ContentBlock::Thinking {
            thinking: "Let's start.".into(),
            signature: None
        }]
    );
}

#[tokio::test]
async fn openai_stream_reasoning_text_and_tool_calls() {
    let chunk = |delta: Value, fr: Value| json!({"id":"c1","object":"chat.completion.chunk","model":"m","choices":[{"index":0,"delta":delta,"finish_reason":fr}]});
    let mut body = sse(&[
        (
            "",
            chunk(json!({"role":"assistant","content":""}), Value::Null),
        ),
        ("", chunk(json!({"reasoning_content":"Need "}), Value::Null)),
        (
            "",
            chunk(json!({"reasoning_content":"files."}), Value::Null),
        ),
        ("", chunk(json!({"content":"Running "}), Value::Null)),
        ("", chunk(json!({"content":"it."}), Value::Null)),
        (
            "",
            chunk(
                json!({"tool_calls":[{"index":0,"id":"call_a","type":"function","function":{"name":"Bash","arguments":""}}]}),
                Value::Null,
            ),
        ),
        (
            "",
            chunk(
                json!({"tool_calls":[{"index":0,"function":{"arguments":"{\"command\":"}}]}),
                Value::Null,
            ),
        ),
        (
            "",
            chunk(
                json!({"tool_calls":[{"index":0,"function":{"arguments":"\"ls\"}"}}]}),
                Value::Null,
            ),
        ),
        (
            "",
            chunk(
                json!({"tool_calls":[{"index":1,"id":"call_b","type":"function","function":{"name":"Bash","arguments":"{\"command\":\"pwd\"}"}}]}),
                Value::Null,
            ),
        ),
        ("", chunk(json!({}), json!("tool_calls"))),
        (
            "",
            json!({"id":"c1","choices":[],"usage":{"prompt_tokens":40,"completion_tokens":12,"total_tokens":52}}),
        ),
    ]);
    body.push_str("data: [DONE]\n\n");
    let (url, cap) = serve("200 OK", "text/event-stream", body).await;
    let p = OpenAi::new(Some("sk-test".into()), Some(format!("{url}/v1")));
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut req = request();
    req.thinking = Some(json!("high"));
    let r = p.complete(req, tx).await.unwrap();

    assert_eq!(r.stop_reason, StopReason::ToolUse);
    assert_eq!(
        r.usage,
        Usage {
            input_tokens: 40,
            output_tokens: 12,
            cache_read_tokens: None,
            cache_write_tokens: None,
        }
    );
    assert_eq!(
        r.message.content,
        vec![
            ContentBlock::Thinking {
                thinking: "Need files.".into(),
                signature: None
            },
            ContentBlock::Text {
                text: "Running it.".into()
            },
            ContentBlock::ToolUse {
                id: "call_a".into(),
                name: "Bash".into(),
                input: json!({"command":"ls"})
            },
            ContentBlock::ToolUse {
                id: "call_b".into(),
                name: "Bash".into(),
                input: json!({"command":"pwd"})
            },
        ]
    );
    assert_eq!(
        drain(rx),
        vec![
            StreamEvent::ThinkingDelta("Need ".into()),
            StreamEvent::ThinkingDelta("files.".into()),
            StreamEvent::TextDelta("Running ".into()),
            StreamEvent::TextDelta("it.".into()),
            StreamEvent::ToolUseStart {
                id: "call_a".into(),
                name: "Bash".into()
            },
            StreamEvent::ToolUseStart {
                id: "call_b".into(),
                name: "Bash".into()
            },
        ]
    );

    let c = cap.await.unwrap();
    assert_eq!(c.path, "/v1/chat/completions");
    assert_eq!(c.header("authorization"), Some("Bearer sk-test"));
    assert!(c.header("x-api-key").is_none());
    assert_eq!(c.body["stream"], true);
    assert_eq!(c.body["stream_options"]["include_usage"], true);
    assert_eq!(c.body["reasoning_effort"], "high");
    assert_eq!(c.body["messages"][0]["role"], "system");
    assert_eq!(c.body["messages"][1]["content"], "list files");
    assert_eq!(c.body["tools"][0]["type"], "function");
    assert_eq!(c.body["tools"][0]["function"]["name"], "Bash");
    assert_eq!(
        c.body["tools"][0]["function"]["parameters"]["type"],
        "object"
    );
}

#[tokio::test]
async fn openai_no_key_sends_no_auth_and_plain_text() {
    let chunk =
        |delta: Value, fr: Value| json!({"choices":[{"index":0,"delta":delta,"finish_reason":fr}]});
    let mut body = sse(&[
        ("", chunk(json!({"content":"hello"}), Value::Null)),
        ("", chunk(json!({}), json!("stop"))),
    ]);
    body.push_str("data: [DONE]\n\n");
    let (url, cap) = serve("200 OK", "text/event-stream", body).await;
    let p = OpenAi::new(None, Some(url));
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let r = p.complete(request(), tx).await.unwrap();
    assert_eq!(r.stop_reason, StopReason::EndTurn);
    assert_eq!(r.message.text(), "hello");
    let c = cap.await.unwrap();
    assert_eq!(c.path, "/chat/completions");
    assert!(c.header("authorization").is_none());
}

#[tokio::test]
async fn openai_error_surfaces_body() {
    let body = r#"{"error":{"message":"model not found","type":"invalid_request_error"}}"#;
    let (url, _cap) = serve("404 Not Found", "application/json", body.into()).await;
    let p = OpenAi::new(None, Some(url));
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let e = p.complete(request(), tx).await.unwrap_err().to_string();
    assert!(e.contains("404"), "{e}");
    assert!(e.contains("model not found"), "{e}");
}

#[tokio::test]
async fn anthropic_stream_without_usage_reports_none() {
    let body = sse(&[
        (
            "message_start",
            json!({"type":"message_start","message":{"id":"m","role":"assistant"}}),
        ),
        (
            "content_block_start",
            json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
        ),
        (
            "content_block_delta",
            json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hi"}}),
        ),
        (
            "message_delta",
            json!({"type":"message_delta","delta":{"stop_reason":"end_turn"}}),
        ),
    ]);
    let (url, _cap) = serve("200 OK", "text/event-stream", body).await;
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let r = Anthropic::new("k", Some(url))
        .complete(request(), tx)
        .await
        .unwrap();
    assert_eq!(r.usage, Usage::default());
    assert!(!r.usage.is_reported());
}

#[tokio::test]
async fn anthropic_stream_usage_with_cache_writes() {
    let body = sse(&[
        (
            "message_start",
            json!({"type":"message_start","message":{"id":"m","role":"assistant","usage":{"input_tokens":3,"cache_read_input_tokens":100,"cache_creation_input_tokens":20,"output_tokens":1}}}),
        ),
        (
            "message_delta",
            json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":9}}),
        ),
    ]);
    let (url, _cap) = serve("200 OK", "text/event-stream", body).await;
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let r = Anthropic::new("k", Some(url))
        .complete(request(), tx)
        .await
        .unwrap();
    assert_eq!(
        r.usage,
        Usage {
            input_tokens: 123,
            output_tokens: 9,
            cache_read_tokens: Some(100),
            cache_write_tokens: Some(20),
        }
    );
}

#[tokio::test]
async fn openai_stream_cached_tokens_and_missing_usage() {
    let chunk = |delta: Value, fr: Value| json!({"id":"c1","choices":[{"index":0,"delta":delta,"finish_reason":fr}]});
    let mut body = sse(&[
        ("", chunk(json!({"content":"hi"}), Value::Null)),
        ("", chunk(json!({}), json!("stop"))),
        (
            "",
            json!({"id":"c1","choices":[],"usage":{"prompt_tokens":100,"completion_tokens":4,"prompt_tokens_details":{"cached_tokens":64}}}),
        ),
    ]);
    body.push_str("data: [DONE]\n\n");
    let (url, _cap) = serve("200 OK", "text/event-stream", body).await;
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let r = OpenAi::new(None, Some(format!("{url}/v1")))
        .complete(request(), tx)
        .await
        .unwrap();
    assert_eq!(
        r.usage,
        Usage {
            input_tokens: 100,
            output_tokens: 4,
            cache_read_tokens: Some(64),
            cache_write_tokens: None,
        }
    );

    // An endpoint that ignores `stream_options.include_usage` sends no usage at all.
    let mut body = sse(&[
        ("", chunk(json!({"content":"hi"}), Value::Null)),
        ("", chunk(json!({}), json!("stop"))),
    ]);
    body.push_str("data: [DONE]\n\n");
    let (url, _cap) = serve("200 OK", "text/event-stream", body).await;
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let r = OpenAi::new(None, Some(format!("{url}/v1")))
        .complete(request(), tx)
        .await
        .unwrap();
    assert!(!r.usage.is_reported());
}

fn no_nulls(v: &Value) {
    match v {
        Value::Null => panic!("request body contains null"),
        Value::Array(a) => a.iter().for_each(no_nulls),
        Value::Object(o) => o.values().for_each(no_nulls),
        _ => {}
    }
}

fn assistant(content: Vec<ContentBlock>) -> Message {
    Message {
        role: ruck_harness_llm::Role::Assistant,
        content,
    }
}

fn openai_ok_body() -> String {
    let mut b = sse(&[(
        "",
        json!({"choices":[{"index":0,"delta":{"content":"ok"},"finish_reason":"stop"}]}),
    )]);
    b.push_str("data: [DONE]\n\n");
    b
}

#[tokio::test]
async fn openai_empty_assistant_content_is_never_null() {
    let (url, cap) = serve("200 OK", "text/event-stream", openai_ok_body()).await;
    let mut req = request();
    req.messages = vec![
        Message::user_text("a"),
        // Thinking only: no text, no tool calls.
        assistant(vec![ContentBlock::Thinking {
            thinking: "hmm".into(),
            signature: None,
        }]),
        Message::user_text("b"),
        // Empty text plus a tool call.
        assistant(vec![
            ContentBlock::Text { text: "".into() },
            ContentBlock::ToolUse {
                id: "t1".into(),
                name: "Bash".into(),
                input: json!({"command": "ls"}),
            },
        ]),
        Message {
            role: ruck_harness_llm::Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "t1".into(),
                content: "".into(),
                is_error: false,
            }],
        },
    ];
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    OpenAi::new(None, Some(url))
        .complete(req, tx)
        .await
        .unwrap();
    let c = cap.await.unwrap();
    no_nulls(&c.body["messages"]);
    let m = &c.body["messages"];
    assert_eq!(m[2]["role"], "assistant");
    assert_eq!(m[2]["content"], "");
    assert!(m[2].get("tool_calls").is_none());
    assert_eq!(m[4]["content"], "");
    assert_eq!(m[4]["tool_calls"][0]["id"], "t1");
    assert_eq!(m[5]["role"], "tool");
    assert_eq!(m[5]["content"], "");
}

#[tokio::test]
async fn anthropic_empty_assistant_turns_send_no_empty_blocks() {
    let body = sse(&[
        (
            "message_start",
            json!({"type":"message_start","message":{"usage":{"input_tokens":1}}}),
        ),
        (
            "message_delta",
            json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":1}}),
        ),
    ]);
    let (url, cap) = serve("200 OK", "text/event-stream", body).await;
    let mut req = request();
    req.messages = vec![
        Message::user_text("a"),
        assistant(vec![ContentBlock::Text { text: "".into() }]),
        Message::user_text("b"),
        // Thinking-only keeps its block and signature.
        assistant(vec![
            ContentBlock::Thinking {
                thinking: "hmm".into(),
                signature: Some("sig".into()),
            },
            ContentBlock::Text { text: "".into() },
        ]),
        Message::user_text("c"),
        assistant(vec![]),
        Message::user_text("d"),
    ];
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    Anthropic::new("k", Some(url))
        .complete(req, tx)
        .await
        .unwrap();
    let c = cap.await.unwrap();
    let msgs = c.body["messages"].as_array().unwrap();
    for m in msgs {
        let blocks = m["content"].as_array().unwrap();
        assert!(!blocks.is_empty(), "empty content array: {m}");
        for b in blocks {
            if b["type"] == "text" {
                assert!(!b["text"].as_str().unwrap().is_empty(), "empty text block");
            }
        }
    }
    let think = msgs
        .iter()
        .find(|m| m["content"][0]["type"] == "thinking")
        .expect("thinking block kept");
    assert_eq!(think["content"][0]["signature"], "sig");
    assert_eq!(think["content"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn openai_extra_body_is_sent_only_when_configured() {
    let extra =
        json!({"presence_penalty": 0.5, "options": {"repeat_penalty": 1.1, "repeat_last_n": 256}});
    let (url, cap) = serve("200 OK", "text/event-stream", openai_ok_body()).await;
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    OpenAi::new(None, Some(url))
        .with_extra_body(extra)
        .complete(request(), tx)
        .await
        .unwrap();
    let c = cap.await.unwrap();
    assert_eq!(c.body["options"]["repeat_penalty"], 1.1);
    assert_eq!(c.body["options"]["repeat_last_n"], 256);
    assert_eq!(c.body["presence_penalty"], 0.5);
    assert_eq!(c.body["model"], "test-model");

    let (url, cap) = serve("200 OK", "text/event-stream", openai_ok_body()).await;
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    OpenAi::new(None, Some(url))
        .complete(request(), tx)
        .await
        .unwrap();
    let c = cap.await.unwrap();
    for k in [
        "options",
        "presence_penalty",
        "frequency_penalty",
        "repeat_penalty",
    ] {
        assert!(c.body.get(k).is_none(), "{k} must not be sent by default");
    }
}

async fn openai_body(req: Request, extra: Option<Value>) -> Value {
    let (url, cap) = serve("200 OK", "text/event-stream", openai_ok_body()).await;
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut p = OpenAi::new(None, Some(url));
    if let Some(e) = extra {
        p = p.with_extra_body(e);
    }
    p.complete(req, tx).await.unwrap();
    cap.await.unwrap().body
}

async fn anthropic_body(req: Request) -> Value {
    let body = sse(&[
        (
            "message_start",
            json!({"type":"message_start","message":{"usage":{"input_tokens":1}}}),
        ),
        (
            "message_delta",
            json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":1}}),
        ),
    ]);
    let (url, cap) = serve("200 OK", "text/event-stream", body).await;
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    Anthropic::new("k", Some(url))
        .complete(req, tx)
        .await
        .unwrap();
    cap.await.unwrap().body
}

#[tokio::test]
async fn openai_sampling_is_sent_only_when_set() {
    let plain = openai_body(request(), None).await;
    assert!(plain.get("temperature").is_none() && plain.get("top_p").is_none());

    let mut req = request();
    req.temperature = Some(0.5);
    req.top_p = Some(0.95);
    let b = openai_body(req, None).await;
    assert_eq!(b["temperature"], 0.5);
    assert_eq!(b["top_p"], 0.95);

    // Setting only one leaves the other absent, and the rest of the body is unchanged.
    let mut req = request();
    req.temperature = Some(0.5);
    let mut b = openai_body(req, None).await;
    assert!(b.get("top_p").is_none());
    b.as_object_mut().unwrap().remove("temperature");
    assert_eq!(b, plain);
}

#[tokio::test]
async fn openai_sampling_combines_with_extra_body_and_wins_duplicates() {
    let extra = json!({"presence_penalty": 0.5, "temperature": 1.0,
                       "options": {"repeat_penalty": 1.1}});
    let mut req = request();
    req.temperature = Some(0.5);
    let b = openai_body(req, Some(extra.clone())).await;
    assert_eq!(b["temperature"], 0.5, "profile temperature wins");
    assert_eq!(b["presence_penalty"], 0.5);
    assert_eq!(b["options"]["repeat_penalty"], 1.1);
    // Without a profile temperature the extra body's value is left alone.
    let b = openai_body(request(), Some(extra)).await;
    assert_eq!(b["temperature"], 1.0);
}

#[tokio::test]
async fn anthropic_sampling_rules() {
    let plain = anthropic_body(request()).await;
    assert!(plain.get("temperature").is_none() && plain.get("top_p").is_none());

    let mut req = request();
    req.temperature = Some(0.5);
    let b = anthropic_body(req).await;
    assert_eq!(b["temperature"], 0.5);
    assert!(b.get("top_p").is_none());

    let mut req = request();
    req.top_p = Some(0.9);
    let b = anthropic_body(req).await;
    assert_eq!(b["top_p"], 0.9);
    assert!(b.get("temperature").is_none());

    // Both set: only temperature is sent.
    let mut req = request();
    req.temperature = Some(0.5);
    req.top_p = Some(0.9);
    let b = anthropic_body(req).await;
    assert_eq!(b["temperature"], 0.5);
    assert!(b.get("top_p").is_none());

    // Thinking enabled: neither is sent.
    let mut req = request();
    req.temperature = Some(0.5);
    req.top_p = Some(0.9);
    req.thinking = Some(json!({"type": "enabled", "budget_tokens": 2000}));
    let b = anthropic_body(req).await;
    assert!(b.get("temperature").is_none() && b.get("top_p").is_none());
    assert_eq!(b["thinking"]["type"], "enabled");
}
