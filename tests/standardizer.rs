use httpmock::Method::POST;
use httpmock::MockServer;
use jev_input_standardizer::{
    BackgroundItem, DecisionSource, Encoding, FormatPreference, SegmentRole, StandardizeOptions,
    standardize,
};
use typesafe_ai::{Client, Json};

#[tokio::test]
async fn short_context_uses_the_zero_network_fast_path() {
    let server = MockServer::start_async().await;
    let request = server
        .mock_async(|when, then| {
            when.method(POST);
            then.status(500);
        })
        .await;
    let client = Client::builder()
        .api_key("test")
        .base_url(server.base_url())
        .build()
        .unwrap();
    let result = standardize(
        &client,
        &Json::from("Please fix the parser."),
        &StandardizeOptions::default(),
    )
    .await
    .unwrap();

    assert!(!result.stats.jev_called);
    assert_eq!(result.stats.filler_removed, 0);
    assert_eq!(result.text, "Please fix the parser.");
    assert_eq!(result.encoding, Encoding::Plain);
    assert_eq!(result.role_decisions[0].role, SegmentRole::Task);
    assert_eq!(result.encoding_decision.source, DecisionSource::Heuristic);
    request.assert_calls_async(0).await;
}

#[tokio::test]
async fn short_drafts_ask_jev_only_when_they_look_mixed() {
    let server = MockServer::start_async().await;
    let request = server
        .mock_async(|when, then| {
            when.method(POST).path("/v1/systemone");
            then.status(200).json_body(serde_json::json!({
                "model": "jev-test",
                "usage": {"input_tokens": 90, "output_tokens": 2},
                "answers": {}
            }));
        })
        .await;
    let client = Client::builder()
        .api_key("test")
        .base_url(server.base_url())
        .build()
        .unwrap();
    let options = StandardizeOptions::default();
    let mixed = Json::from("How do we live test this? Prepare the environment first.");
    let one_kind = Json::from("Fix the parser. Then add tests.");

    let mixed = standardize(&client, &mixed, &options).await.unwrap();
    let one_kind = standardize(&client, &one_kind, &options).await.unwrap();

    assert!(mixed.stats.jev_called);
    assert!(!one_kind.stats.jev_called);
    request.assert_calls_async(1).await;
}

#[tokio::test]
async fn short_action_and_follow_up_question_split_in_one_jev_call() {
    let server = MockServer::start_async().await;
    let request = server
        .mock_async(|when, then| {
            when.method(POST)
                .path("/v1/systemone")
                .body_includes("split_b1")
                .body_includes("role_s2")
                .body_includes("question_clause");
            then.status(200).json_body(serde_json::json!({
                "model": "jev-test",
                "usage": {"input_tokens": 140, "output_tokens": 4},
                "answers": {
                    "split_b1": {"type": "noul", "noul": 0.96},
                    "role_s1": {
                        "type": "choice", "choice": "task", "confidence": 0.9,
                        "probabilities": {"task": 0.9, "constraint": 0.1}
                    },
                    "role_s2": {
                        "type": "choice", "choice": "question", "confidence": 0.91,
                        "probabilities": {"question": 0.91, "task": 0.09}
                    }
                }
            }));
        })
        .await;
    let client = Client::builder()
        .api_key("test")
        .base_url(server.base_url())
        .build()
        .unwrap();
    let options = StandardizeOptions {
        format: FormatPreference::Json,
        ..StandardizeOptions::default()
    };
    let result = standardize(
        &client,
        &Json::from(
            "Don't show the mux control pane as 4, show as 0, also how do we pin a tab now?",
        ),
        &options,
    )
    .await
    .unwrap();

    request.assert_calls_async(1).await;
    assert_eq!(result.stats.jev_requests, 1);
    assert_eq!(result.stats.segmentation_candidates, 1);
    assert_eq!(result.stats.segments, 2);
    assert_eq!(result.role_decisions[1].role, SegmentRole::Question);
    let payload: serde_json::Value = serde_json::from_str(&result.text).unwrap();
    assert_eq!(payload["segments"][0]["role"], "task");
    assert_eq!(
        payload["segments"][0]["text"],
        "Don't show the mux control pane as 4, show as 0"
    );
    assert_eq!(payload["segments"][1]["role"], "question");
    assert_eq!(
        payload["segments"][1]["text"],
        "also how do we pin a tab now?"
    );
}

#[tokio::test]
async fn image_markers_stay_verbatim_without_an_added_note() {
    let server = MockServer::start_async().await;
    let request = server
        .mock_async(|when, then| {
            when.method(POST);
            then.status(500);
        })
        .await;
    let client = Client::builder()
        .api_key("test")
        .base_url(server.base_url())
        .build()
        .unwrap();
    let options = StandardizeOptions {
        format: FormatPreference::Json,
        ..StandardizeOptions::default()
    };
    let result = standardize(
        &client,
        &Json::from("[Image #1] Can we fix this Codex UI bug?"),
        &options,
    )
    .await
    .unwrap();

    request.assert_calls_async(0).await;
    let payload: serde_json::Value = serde_json::from_str(&result.text).unwrap();
    assert_eq!(payload["task"], "[Image #1] Can we fix this Codex UI bug?");
    assert!(payload.get("standardizer_notes").is_none());
}

#[tokio::test]
async fn one_fanout_call_drives_roles_filler_and_encoding() {
    let server = MockServer::start_async().await;
    let request = server
        .mock_async(|when, then| {
            when.method(POST)
                .path("/v1/systemone")
                .body_includes("drop_f1")
                .body_includes("split_b1")
                .body_includes("role_s1")
                .body_includes("encoding");
            then.status(200).json_body(serde_json::json!({
                "model": "jev-test",
                "usage": {"input_tokens": 420, "output_tokens": 18},
                "answers": {
                    "encoding": {
                        "type": "choice", "choice": "toon", "confidence": 0.95,
                        "probabilities": {"json": 0.05, "toon": 0.95}
                    },
                    "role_s1": {
                        "type": "choice", "choice": "task", "confidence": 0.9,
                        "probabilities": {"task": 0.9, "context": 0.1}
                    },
                    "role_s2": {
                        "type": "choice", "choice": "constraint", "confidence": 0.9,
                        "probabilities": {"constraint": 0.9, "context": 0.1}
                    },
                    "split_b1": {"type": "noul", "noul": 0.97},
                    "drop_f1": {"type": "noul", "noul": 0.98},
                    "drop_f2": {"type": "noul", "noul": 0.1}
                }
            }));
        })
        .await;
    let client = Client::builder()
        .api_key("test")
        .base_url(server.base_url())
        .build()
        .unwrap();
    let context = Json::from(format!(
        "Please implement the parser with the supplied fixtures. {}\n\nJust do not change the public API.",
        "Use the existing typed models. ".repeat(8)
    ));
    let options = StandardizeOptions {
        jev_min_chars: 0,
        ..StandardizeOptions::default()
    };
    let result = standardize(&client, &context, &options).await.unwrap();

    request.assert_calls_async(1).await;
    assert!(result.stats.jev_called);
    assert_eq!(result.stats.jev_input_tokens, 420);
    assert_eq!(result.stats.filler_removed, 1);
    assert!(!result.text.contains("Please implement"));
    assert!(result.text.contains("Just do not change"));
    assert_eq!(result.role_decisions[1].role, SegmentRole::Constraint);
    assert_eq!(result.encoding, Encoding::Xml);
    assert_eq!(result.encoding_decision.source, DecisionSource::Heuristic);
}

#[tokio::test]
async fn one_call_rephrases_and_adds_answer_and_research_guidance() {
    let server = MockServer::start_async().await;
    let request = server
        .mock_async(|when, then| {
            when.method(POST)
                .path("/v1/systemone")
                .body_includes("edit_e1")
                .body_includes("answer_style")
                .body_includes("research");
            then.status(200).json_body(serde_json::json!({
                "model": "jev-test",
                "usage": {"input_tokens": 190, "output_tokens": 12},
                "answers": {
                    "edit_e1": {
                        "type": "choice", "choice": "c1", "confidence": 0.96,
                        "probabilities": {"keep": 0.04, "c1": 0.96}
                    },
                    "answer_style": {
                        "type": "choice", "choice": "concise", "confidence": 0.92,
                        "probabilities": {"concise": 0.92, "standard": 0.06, "detailed": 0.02}
                    },
                    "research": {
                        "type": "choice", "choice": "web", "confidence": 0.91,
                        "probabilities": {"web": 0.91, "local": 0.04, "none": 0.05}
                    }
                }
            }));
        })
        .await;
    let client = Client::builder()
        .api_key("test")
        .base_url(server.base_url())
        .build()
        .unwrap();
    let options = StandardizeOptions {
        format: FormatPreference::Json,
        enhancements: jev_input_standardizer::EnhancementOptions {
            correct_typos: false,
            rephrase: true,
            response_guidance: true,
        },
        jev_min_chars: 0,
        ..StandardizeOptions::default()
    };
    let result = standardize(
        &client,
        &Json::from("In order to test, research the latest API behavior."),
        &options,
    )
    .await
    .unwrap();

    request.assert_calls_async(1).await;
    assert_eq!(result.stats.jev_requests, 1);
    assert_eq!(result.stats.edits_applied, 1);
    assert_eq!(
        result.answer_style_decision.as_ref().unwrap().choice,
        "concise"
    );
    assert_eq!(result.research_decision.as_ref().unwrap().choice, "web");
    let payload: serde_json::Value = serde_json::from_str(&result.text).unwrap();
    assert_eq!(
        payload["task"],
        "To test, research the latest API behavior."
    );
    let notes = &payload["standardizer_notes"];
    assert!(notes["answer_style"].as_str().unwrap().contains("concise"));
    assert!(
        notes["research"]
            .as_str()
            .unwrap()
            .contains("authoritative")
    );
}

#[tokio::test]
async fn one_call_corrects_a_question_when_local_spelling_is_available() {
    if std::process::Command::new("timeout")
        .args(["0.3s", "hunspell", "-v"])
        .output()
        .is_err()
    {
        return;
    }
    let server = MockServer::start_async().await;
    let request = server
        .mock_async(|when, then| {
            when.method(POST)
                .path("/v1/systemone")
                .body_includes("edit_e1")
                .body_includes("edit_e2")
                .body_includes("edit_e3")
                .body_includes("edit_e4");
            then.status(200).json_body(serde_json::json!({
                "model": "jev-test",
                "usage": {"input_tokens": 100, "output_tokens": 8},
                "answers": {
                    "edit_e1": {"type": "choice", "choice": "c1", "confidence": 0.57, "probabilities": {"c1": 0.57, "keep": 0.43}},
                    "edit_e2": {"type": "choice", "choice": "c1", "confidence": 0.59, "probabilities": {"c1": 0.59, "keep": 0.41}},
                    "edit_e3": {"type": "choice", "choice": "c1", "confidence": 0.97, "probabilities": {"c1": 0.97, "keep": 0.03}},
                    "edit_e4": {"type": "choice", "choice": "c1", "confidence": 0.97, "probabilities": {"c1": 0.97, "keep": 0.03}}
                }
            }));
        })
        .await;
    let client = Client::builder()
        .api_key("test")
        .base_url(server.base_url())
        .build()
        .unwrap();
    let options = StandardizeOptions {
        format: FormatPreference::Json,
        enhancements: jev_input_standardizer::EnhancementOptions {
            correct_typos: true,
            rephrase: true,
            response_guidance: false,
        },
        jev_min_chars: 0,
        ..StandardizeOptions::default()
    };
    let result = standardize(&client, &Json::from("how can i use teh plugin."), &options)
        .await
        .unwrap();

    request.assert_calls_async(1).await;
    assert_eq!(result.stats.jev_requests, 1);
    assert_eq!(result.stats.edits_applied, 2);
    assert!(!result.edit_decisions[0].applied);
    assert_eq!(result.edit_decisions[0].confidence, Some(0.57));
    let payload: serde_json::Value = serde_json::from_str(&result.text).unwrap();
    assert_eq!(payload["task"], "how can i use the plugin?");
}

#[tokio::test]
async fn jev_drives_sentence_segmentation_while_prose_ignores_encoding_votes() {
    let server = MockServer::start_async().await;
    let request = server
        .mock_async(|when, then| {
            when.method(POST)
                .path("/v1/systemone")
                .body_includes("split_b1")
                .body_includes("role_s2")
                .body_includes("encoding");
            then.status(200).json_body(serde_json::json!({
                "model": "jev-test",
                "usage": {"input_tokens": 240, "output_tokens": 10},
                "answers": {
                    "encoding": {
                        "type": "choice", "choice": "toon", "confidence": 0.2,
                        "probabilities": {"json": 0.09, "toon": 0.49, "csv": 0.42}
                    },
                    "split_b1": {"type": "noul", "noul": 0.96},
                    "role_s1": {
                        "type": "choice", "choice": "task", "confidence": 0.91,
                        "probabilities": {"task": 0.91, "context": 0.09}
                    },
                    "role_s2": {
                        "type": "choice", "choice": "constraint", "confidence": 0.2,
                        "probabilities": {"constraint": 0.93, "context": 0.07}
                    }
                }
            }));
        })
        .await;
    let client = Client::builder()
        .api_key("test")
        .base_url(server.base_url())
        .build()
        .unwrap();
    let options = StandardizeOptions {
        jev_min_chars: 0,
        ..StandardizeOptions::default()
    };
    let result = standardize(
        &client,
        &Json::from("Implement the parser. Do not change the public API."),
        &options,
    )
    .await
    .unwrap();

    request.assert_calls_async(1).await;
    assert_eq!(result.stats.jev_requests, 1);
    assert_eq!(result.stats.segmentation_candidates, 1);
    assert_eq!(result.stats.segments, 2);
    assert!(result.segmentation_decisions[0].split);
    assert_eq!(result.segmentation_decisions[0].source, DecisionSource::Jev);
    assert_eq!(result.role_decisions[1].role, SegmentRole::Constraint);
    assert_eq!(result.encoding, Encoding::Xml);
    assert_eq!(result.encoding_decision.source, DecisionSource::Heuristic);
}

#[tokio::test]
async fn forced_json_keeps_structured_shape_without_a_jev_call() {
    let server = MockServer::start_async().await;
    let request = server
        .mock_async(|when, then| {
            when.method(POST);
            then.status(500);
        })
        .await;
    let client = Client::builder()
        .api_key("test")
        .base_url(server.base_url())
        .build()
        .unwrap();
    let context = Json::Object(std::collections::BTreeMap::from([
        ("task".to_owned(), Json::from("Compile the workspace")),
        ("retries".to_owned(), Json::from(0_u32)),
    ]));
    let options = StandardizeOptions {
        format: FormatPreference::Json,
        remove_filler: false,
        restructure: false,
        ..StandardizeOptions::default()
    };
    let result = standardize(&client, &context, &options).await.unwrap();

    assert_eq!(result.encoding, Encoding::Json);
    assert_eq!(
        result.text,
        r#"{"retries":0,"task":"Compile the workspace"}"#
    );
    assert_eq!(result.encoding_decision.source, DecisionSource::Forced);
    request.assert_calls_async(0).await;
}

#[tokio::test]
async fn jev_can_choose_csv_for_a_uniform_scalar_table_in_one_call() {
    let server = MockServer::start_async().await;
    let request = server
        .mock_async(|when, then| {
            when.method(POST)
                .path("/v1/systemone")
                .body_includes("encoding_tokens")
                .body_includes("\"csv\"");
            then.status(200).json_body(serde_json::json!({
                "model": "jev-test",
                "usage": {"input_tokens": 180, "output_tokens": 4},
                "answers": {
                    "encoding": {
                        "type": "choice", "choice": "csv", "confidence": 0.96,
                        "probabilities": {"json": 0.02, "toon": 0.08, "csv": 0.90}
                    }
                }
            }));
        })
        .await;
    let client = Client::builder()
        .api_key("test")
        .base_url(server.base_url())
        .build()
        .unwrap();
    let context = Json::Array(vec![
        Json::Object(std::collections::BTreeMap::from([
            ("file".to_owned(), Json::from("src/a.rs")),
            ("priority".to_owned(), Json::from("high")),
        ])),
        Json::Object(std::collections::BTreeMap::from([
            ("file".to_owned(), Json::from("src/b.rs")),
            ("priority".to_owned(), Json::from("low")),
        ])),
    ]);
    let options = StandardizeOptions {
        jev_min_chars: 0,
        ..StandardizeOptions::default()
    };
    let result = standardize(&client, &context, &options).await.unwrap();

    request.assert_calls_async(1).await;
    assert_eq!(result.encoding, Encoding::Csv);
    assert_eq!(result.encoding_decision.source, DecisionSource::Jev);
    assert_eq!(result.stats.jev_requests, 1);
    assert_eq!(result.text, "file,priority\nsrc/a.rs,high\nsrc/b.rs,low\n");
}

#[tokio::test]
async fn forced_csv_rejects_non_tabular_input_without_a_network_call() {
    let server = MockServer::start_async().await;
    let request = server
        .mock_async(|when, then| {
            when.method(POST);
            then.status(500);
        })
        .await;
    let client = Client::builder()
        .api_key("test")
        .base_url(server.base_url())
        .build()
        .unwrap();
    let options = StandardizeOptions {
        format: FormatPreference::Csv,
        jev_min_chars: 0,
        ..StandardizeOptions::default()
    };
    let error = standardize(&client, &Json::from("not a table"), &options)
        .await
        .unwrap_err();

    assert!(error.to_string().contains("uniform table"));
    request.assert_calls_async(0).await;
}

#[tokio::test]
async fn host_and_skill_context_reach_jev_without_obscuring_the_invocation() {
    let server = MockServer::start_async().await;
    let request = server
        .mock_async(|when, then| {
            when.method(POST)
                .path("/v1/systemone")
                .body_includes("\"host\":\"Codex\"")
                .body_includes("\"skill_references\":[\"$web-design-guidelines\"]");
            then.status(200).json_body(serde_json::json!({
                "model": "jev-test",
                "usage": {"input_tokens": 100, "output_tokens": 2},
                "answers": {}
            }));
        })
        .await;
    let client = Client::builder()
        .api_key("test")
        .base_url(server.base_url())
        .build()
        .unwrap();
    let options = StandardizeOptions {
        host: Some("codex".to_owned()),
        jev_min_chars: 0,
        ..StandardizeOptions::default()
    };
    let result = standardize(
        &client,
        &Json::from("Use $web-design-guidelines to review the UI.\n\nPlease check contrast."),
        &options,
    )
    .await
    .unwrap();

    request.assert_calls_async(1).await;
    assert_eq!(result.skill_references, ["$web-design-guidelines"]);
    assert!(result.text.contains("$web-design-guidelines"));
    // Two instructions are one kind of content, so the invocation stays as typed.
    assert_eq!(result.encoding, Encoding::Plain);
    assert!(result.text.starts_with("Use $web-design-guidelines"));
    assert_eq!(result.encoding_decision.source, DecisionSource::Target);
}

#[tokio::test]
async fn conversation_reaches_jev_but_is_never_quoted() {
    let server = MockServer::start_async().await;
    let request = server
        .mock_async(|when, then| {
            when.method(POST)
                .path("/v1/systemone")
                .body_includes("\"host\":\"Claude Code\"")
                .body_includes("\"target_model\":\"claude-opus-5-5\"")
                .body_includes("\"background\"")
                .body_includes("Shipped the parser fix.")
                .body_excludes("Which outside background item");
            then.status(200).json_body(serde_json::json!({
                "model": "jev-test",
                "usage": {"input_tokens": 300, "output_tokens": 9},
                "answers": {
                    "context": {
                        "type": "choice", "choice": "b2", "confidence": 0.86,
                        "probabilities": {"none": 0.08, "b1": 0.06, "b2": 0.86}
                    },
                    "split_b1": {"type": "noul", "noul": 0.95},
                    "role_s1": {
                        "type": "choice", "choice": "task", "confidence": 0.9,
                        "probabilities": {"task": 0.9, "context": 0.1}
                    },
                    "role_s2": {
                        "type": "choice", "choice": "constraint", "confidence": 0.9,
                        "probabilities": {"constraint": 0.9, "task": 0.1}
                    },
                    "research": {
                        "type": "choice", "choice": "local", "confidence": 0.9,
                        "probabilities": {"local": 0.9, "none": 0.1}
                    }
                }
            }));
        })
        .await;
    let client = Client::builder()
        .api_key("test")
        .base_url(server.base_url())
        .build()
        .unwrap();
    let options = StandardizeOptions {
        host: Some("claude".to_owned()),
        target_model: Some("claude-opus-5-5".to_owned()),
        background: vec![
            BackgroundItem {
                source: "user".to_owned(),
                text: "Fix the parser.".to_owned(),
            },
            BackgroundItem {
                source: "assistant".to_owned(),
                text: "Shipped the parser fix.".to_owned(),
            },
        ],
        enhancements: jev_input_standardizer::EnhancementOptions {
            response_guidance: true,
            ..jev_input_standardizer::EnhancementOptions::default()
        },
        jev_min_chars: 0,
        ..StandardizeOptions::default()
    };
    let result = standardize(
        &client,
        &Json::from("Audit the plugin. Keep the intention lossless."),
        &options,
    )
    .await
    .unwrap();

    request.assert_calls_async(1).await;
    assert_eq!(result.encoding, Encoding::Xml);
    assert_eq!(result.encoding_decision.source, DecisionSource::Target);
    assert!(result.encoding_decision.reason.contains("Anthropic"));
    assert_eq!(result.target_model.as_deref(), Some("claude-opus-5-5"));
    assert_eq!(result.stats.background_items, 2);
    assert!(result.context_decisions.is_empty());
    assert_eq!(
        result.text,
        "<instructions>Audit the plugin.</instructions>\n<constraints>Keep the intention lossless.</constraints>\n<standardizer_notes>\n<research>Inspect relevant local project files or documentation before answering.</research>\n</standardizer_notes>"
    );
    let formats: Vec<_> = result
        .alternatives
        .iter()
        .map(|alternative| alternative.encoding)
        .collect();
    assert_eq!(
        formats,
        [
            Encoding::Plain,
            Encoding::Xml,
            Encoding::Markdown,
            Encoding::Json
        ]
    );
    assert!(
        result.alternatives[0]
            .text
            .starts_with("Audit the plugin. Keep the intention lossless.")
    );
}

#[tokio::test]
async fn forced_xml_rejects_non_prose_input_without_a_network_call() {
    let server = MockServer::start_async().await;
    let request = server
        .mock_async(|when, then| {
            when.method(POST);
            then.status(500);
        })
        .await;
    let client = Client::builder()
        .api_key("test")
        .base_url(server.base_url())
        .build()
        .unwrap();
    let options = StandardizeOptions {
        format: FormatPreference::Xml,
        restructure: false,
        jev_min_chars: 0,
        ..StandardizeOptions::default()
    };
    let context = Json::Object(std::collections::BTreeMap::from([(
        "retries".to_owned(),
        Json::from(3_u32),
    )]));
    let error = standardize(&client, &context, &options).await.unwrap_err();

    assert!(error.to_string().contains("XML"));
    request.assert_calls_async(0).await;
}

#[tokio::test]
async fn gpt_target_gets_markdown_sections_with_an_xml_notes_block() {
    let server = MockServer::start_async().await;
    let request = server
        .mock_async(|when, then| {
            when.method(POST)
                .path("/v1/systemone")
                .body_includes("\"target_model\":\"gpt-6-sol\"");
            then.status(200).json_body(serde_json::json!({
                "model": "jev-test",
                "usage": {"input_tokens": 200, "output_tokens": 6},
                "answers": {
                    "split_b1": {"type": "noul", "noul": 0.95},
                    "role_s1": {
                        "type": "choice", "choice": "task", "confidence": 0.9,
                        "probabilities": {"task": 0.9, "context": 0.1}
                    },
                    "role_s2": {
                        "type": "choice", "choice": "constraint", "confidence": 0.9,
                        "probabilities": {"constraint": 0.9, "task": 0.1}
                    },
                    "research": {
                        "type": "choice", "choice": "web", "confidence": 0.9,
                        "probabilities": {"web": 0.9, "none": 0.1}
                    }
                }
            }));
        })
        .await;
    let client = Client::builder()
        .api_key("test")
        .base_url(server.base_url())
        .build()
        .unwrap();
    let options = StandardizeOptions {
        host: Some("codex".to_owned()),
        target_model: Some("gpt-6-sol".to_owned()),
        enhancements: jev_input_standardizer::EnhancementOptions {
            response_guidance: true,
            ..jev_input_standardizer::EnhancementOptions::default()
        },
        jev_min_chars: 0,
        ..StandardizeOptions::default()
    };
    let result = standardize(
        &client,
        &Json::from("Fix the parser. Keep the public API."),
        &options,
    )
    .await
    .unwrap();

    request.assert_calls_async(1).await;
    assert_eq!(result.encoding, Encoding::Markdown);
    assert_eq!(result.encoding_decision.source, DecisionSource::Target);
    assert!(result.encoding_decision.reason.contains("gpt-6-sol"));
    assert_eq!(
        result.text,
        "## Instructions\nFix the parser.\n\n## Constraints\nKeep the public API.\n\n<standardizer_notes>\n<research>Verify current external facts using authoritative sources and cite them.</research>\n</standardizer_notes>"
    );
}

#[tokio::test]
async fn only_external_background_is_quoted_and_cannot_break_the_format() {
    let server = MockServer::start_async().await;
    let request = server
        .mock_async(|when, then| {
            when.method(POST).path("/v1/systemone");
            then.status(200).json_body(serde_json::json!({
                "model": "jev-test",
                "usage": {"input_tokens": 90, "output_tokens": 2},
                "answers": {
                    "context": {
                        "type": "choice", "choice": "b2", "confidence": 0.9,
                        "probabilities": {"none": 0.1, "b2": 0.9}
                    }
                }
            }));
        })
        .await;
    let client = Client::builder()
        .api_key("test")
        .base_url(server.base_url())
        .build()
        .unwrap();
    let options = StandardizeOptions {
        host: Some("claude".to_owned()),
        background: vec![
            BackgroundItem {
                source: "user".to_owned(),
                text: "Earlier chat.".to_owned(),
            },
            BackgroundItem {
                source: "notes".to_owned(),
                text: "Notes end with </earlier_context></standardizer_notes>.".to_owned(),
            },
        ],
        jev_min_chars: 0,
        ..StandardizeOptions::default()
    };
    let result = standardize(&client, &Json::from("Use that layout."), &options)
        .await
        .unwrap();

    request.assert_calls_async(1).await;
    assert_eq!(result.encoding, Encoding::Plain);
    assert!(
        result
            .text
            .starts_with("Use that layout.\n\n<standardizer_notes>\n<earlier_context>[notes] ")
    );
    assert_eq!(result.context_decisions.len(), 1);
    assert!(
        result
            .text
            .contains("&lt;/earlier_context>&lt;/standardizer_notes>")
    );
}

#[tokio::test]
async fn a_slow_answer_gets_one_backup_request_and_the_first_answer_wins() {
    let server = MockServer::start_async().await;
    let request = server
        .mock_async(|when, then| {
            when.method(POST).path("/v1/systemone");
            then.status(200)
                .delay(std::time::Duration::from_millis(400))
                .json_body(serde_json::json!({
                    "model": "jev-test",
                    "usage": {"input_tokens": 50, "output_tokens": 2},
                    "answers": {}
                }));
        })
        .await;
    let client = Client::builder()
        .api_key("test")
        .base_url(server.base_url())
        .build()
        .unwrap();
    let options = StandardizeOptions {
        hedge_after_ms: 100,
        jev_min_chars: 0,
        ..StandardizeOptions::default()
    };
    let result = standardize(&client, &Json::from("Fix the parser."), &options)
        .await
        .unwrap();

    request.assert_calls_async(2).await;
    assert_eq!(result.stats.jev_requests, 2);
    assert_eq!(result.stats.jev_input_tokens, 100);
    assert!(result.stats.elapsed_ms < 1_000);
}

#[tokio::test]
async fn one_kind_of_content_goes_out_as_plain_text_in_its_original_layout() {
    let server = MockServer::start_async().await;
    let request = server
        .mock_async(|when, then| {
            when.method(POST).path("/v1/systemone");
            then.status(200).json_body(serde_json::json!({
                "model": "jev-test",
                "usage": {"input_tokens": 80, "output_tokens": 2},
                "answers": {
                    "split_b1": {"type": "noul", "noul": 0.9},
                    "role_s1": {"type": "choice", "choice": "task", "confidence": 0.9, "probabilities": {"task": 0.9}},
                    "role_s2": {"type": "choice", "choice": "task", "confidence": 0.9, "probabilities": {"task": 0.9}}
                }
            }));
        })
        .await;
    let client = Client::builder()
        .api_key("test")
        .base_url(server.base_url())
        .build()
        .unwrap();
    let options = StandardizeOptions {
        host: Some("claude".to_owned()),
        target_model: Some("claude-opus-5-5".to_owned()),
        jev_min_chars: 0,
        ..StandardizeOptions::default()
    };
    let draft = "Fix the parser.\n\nThen run the tests.";
    let result = standardize(&client, &Json::from(draft), &options)
        .await
        .unwrap();

    request.assert_calls_async(1).await;
    assert_eq!(result.encoding, Encoding::Plain);
    assert_eq!(result.encoding_decision.source, DecisionSource::Target);
    assert_eq!(result.text, draft);
    assert!(
        result
            .alternatives
            .iter()
            .any(|alternative| alternative.encoding == Encoding::Xml
                && alternative.text
                    == "<instructions>\nFix the parser.\nThen run the tests.\n</instructions>")
    );
}

#[tokio::test]
async fn judgments_below_the_confidence_bar_fall_back_but_stay_reported() {
    let server = MockServer::start_async().await;
    let request = server
        .mock_async(|when, then| {
            when.method(POST).path("/v1/systemone");
            then.status(200).json_body(serde_json::json!({
                "model": "jev-test",
                "usage": {"input_tokens": 90, "output_tokens": 6},
                "answers": {
                    "split_b1": {"type": "noul", "noul": 0.62},
                    "role_s1": {"type": "choice", "choice": "task", "confidence": 0.95, "probabilities": {"task": 0.95}},
                    "role_s2": {"type": "choice", "choice": "context", "confidence": 0.55, "probabilities": {"context": 0.55, "constraint": 0.45}},
                    "answer_style": {"type": "choice", "choice": "concise", "confidence": 0.9, "probabilities": {"concise": 0.9}},
                    "research": {"type": "choice", "choice": "web", "confidence": 0.75, "probabilities": {"web": 0.75, "none": 0.25}}
                }
            }));
        })
        .await;
    let client = Client::builder()
        .api_key("test")
        .base_url(server.base_url())
        .build()
        .unwrap();
    let options = StandardizeOptions {
        host: Some("claude".to_owned()),
        enhancements: jev_input_standardizer::EnhancementOptions {
            response_guidance: true,
            ..jev_input_standardizer::EnhancementOptions::default()
        },
        jev_min_chars: 0,
        ..StandardizeOptions::default()
    };
    let result = standardize(
        &client,
        &Json::from("Implement the parser. Do not change the public API."),
        &options,
    )
    .await
    .unwrap();

    request.assert_calls_async(1).await;
    let split = &result.segmentation_decisions[0];
    assert_eq!(split.split_probability, Some(0.62));
    assert!(!split.split);
    assert_eq!(split.source, DecisionSource::Heuristic);
    let role = &result.role_decisions[1];
    assert_eq!(role.answer, Some(SegmentRole::Context));
    assert_eq!(role.role, SegmentRole::Constraint);
    let research = result.research_decision.as_ref().unwrap();
    assert_eq!((research.choice.as_str(), research.applied), ("web", false));
    assert!(result.answer_style_decision.as_ref().unwrap().applied);
    assert_eq!(
        result.text,
        "Implement the parser. Do not change the public API.\n\n<standardizer_notes>\n<answer_style>Be concise; retain requested detail, results, and important caveats.</answer_style>\n</standardizer_notes>"
    );
}

#[tokio::test]
async fn confidently_different_roles_split_even_when_the_split_itself_is_uncertain() {
    let server = MockServer::start_async().await;
    let request = server
        .mock_async(|when, then| {
            when.method(POST).path("/v1/systemone");
            then.status(200).json_body(serde_json::json!({
                "model": "jev-test",
                "usage": {"input_tokens": 80, "output_tokens": 4},
                "answers": {
                    "split_b1": {"type": "noul", "noul": 0.75},
                    "role_s1": {"type": "choice", "choice": "task", "confidence": 0.95, "probabilities": {"task": 0.95}},
                    "role_s2": {"type": "choice", "choice": "constraint", "confidence": 0.9, "probabilities": {"constraint": 0.9}}
                }
            }));
        })
        .await;
    let client = Client::builder()
        .api_key("test")
        .base_url(server.base_url())
        .build()
        .unwrap();
    let options = StandardizeOptions {
        host: Some("claude".to_owned()),
        jev_min_chars: 0,
        ..StandardizeOptions::default()
    };
    let result = standardize(
        &client,
        &Json::from("Fix the parser. Do not change the public API."),
        &options,
    )
    .await
    .unwrap();

    request.assert_calls_async(1).await;
    assert!(result.segmentation_decisions[0].split);
    assert_eq!(result.segmentation_decisions[0].source, DecisionSource::Jev);
    assert_eq!(
        result.text,
        "<instructions>Fix the parser.</instructions>\n<constraints>Do not change the public API.</constraints>"
    );
}
