use super::*;

async fn seeded_service() -> (LiveSessionService, Arc<SessionStore>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(SessionStore::new(dir.path().to_path_buf()));
    let metadata = Arc::new(SqliteSessionMetadata::new(sqlite_pool().await));
    metadata.upsert("main", None).await.unwrap();
    for (role, content) in [
        ("user", "one"),
        ("assistant", "first reply"),
        ("user", "two"),
        ("assistant", "second reply"),
    ] {
        store
            .append(
                "main",
                &serde_json::json!({ "role": role, "content": content }),
            )
            .await
            .unwrap();
    }
    let svc = LiveSessionService::new(Arc::clone(&store), metadata);
    (svc, store, dir)
}

#[tokio::test]
async fn truncate_keeps_the_first_messages() {
    let (svc, store, _dir) = seeded_service().await;
    let result = svc
        .truncate(serde_json::json!({ "key": "main", "keep": 2 }))
        .await
        .unwrap();
    assert_eq!(result["messageCount"], 2);
    assert_eq!(result["removed"], 2);
    let messages = store.read("main").await.unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[1]["content"], "first reply");
}

#[tokio::test]
async fn truncate_to_current_length_is_a_no_op() {
    let (svc, store, _dir) = seeded_service().await;
    let result = svc
        .truncate(serde_json::json!({ "key": "main", "keep": 4 }))
        .await
        .unwrap();
    assert_eq!(result["removed"], 0);
    assert_eq!(store.read("main").await.unwrap().len(), 4);
}

#[tokio::test]
async fn truncate_rejects_keep_beyond_history_and_missing_params() {
    let (svc, store, _dir) = seeded_service().await;
    assert!(
        svc.truncate(serde_json::json!({ "key": "main", "keep": 9 }))
            .await
            .is_err()
    );
    assert!(
        svc.truncate(serde_json::json!({ "key": "main" }))
            .await
            .is_err()
    );
    assert_eq!(store.read("main").await.unwrap().len(), 4);
}

#[test]
fn rewind_points_group_runs_under_their_user_message() {
    let messages = vec![
        serde_json::json!({"role": "user", "content": "one", "run_id": "r1"}),
        serde_json::json!({"role": "assistant", "content": "", "run_id": "r1"}),
        serde_json::json!({"role": "tool_result", "run_id": "r1"}),
        serde_json::json!({"role": "assistant", "content": "done", "run_id": "r1"}),
        serde_json::json!({"role": "notice", "content": "compacted"}),
        serde_json::json!({"role": "user", "content": [
            {"type": "text", "text": "look"},
            {"type": "image_url", "image_url": {"url": "data:"}}
        ]}),
        serde_json::json!({"role": "assistant", "content": "a cat", "run_id": "r2"}),
    ];
    let points = maintenance::rewind_points(&messages);
    assert_eq!(points.len(), 2);
    assert_eq!(points[0].index, 0);
    assert_eq!(points[0].text, "one");
    assert_eq!(points[0].run_ids, vec!["r1".to_string()]);
    assert!(!points[0].has_attachments);
    assert_eq!(points[1].index, 5);
    assert_eq!(points[1].text, "look");
    assert!(points[1].has_attachments);
    assert_eq!(points[1].run_ids, vec!["r2".to_string()]);
}

#[tokio::test]
async fn rewind_points_rpc_lists_user_turns() {
    let (svc, _store, _dir) = seeded_service().await;
    let result = svc
        .rewind_points(serde_json::json!({ "key": "main" }))
        .await
        .unwrap();
    assert_eq!(result["messageCount"], 4);
    let points = result["points"].as_array().unwrap();
    assert_eq!(points.len(), 2);
    assert_eq!(points[1]["index"], 2);
    assert_eq!(points[1]["text"], "two");
}
