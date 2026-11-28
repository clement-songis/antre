#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use {
    super::*,
    crate::{
        events::{Visibility, tests::RecordingSink},
        publish::tests::MemoryShares,
        render::tests::FakeRenderer,
        store::tests::test_pool,
    },
};

/// A service on a temp data dir with seeded design systems, an in-memory
/// share store and a fake renderer (unless one is given).
pub(crate) async fn service_with(
    renderer: Option<Arc<dyn DesignRenderer>>,
) -> (tempfile::TempDir, Arc<CanvasService>, Arc<RecordingSink>) {
    let tmp = tempfile::tempdir().unwrap();
    crate::design_systems::seed_starters(&tmp.path().join("skills")).unwrap();
    let renderer = renderer.unwrap_or_else(|| Arc::new(FakeRenderer::default()));
    let service = CanvasService::new(CanvasServiceParts {
        data_dir: tmp.path().to_path_buf(),
        pool: test_pool().await,
        settings: CanvasSettings::default(),
        renderer: Some(renderer),
        shares: Some(Arc::new(MemoryShares::default())),
    })
    .unwrap();
    service.set_render_address("127.0.0.1:18790".parse().unwrap());
    let sink = Arc::new(RecordingSink::default());
    service.set_event_sink(Arc::clone(&sink) as Arc<dyn CanvasEventSink>);
    (tmp, Arc::new(service), sink)
}

async fn create(service: &CanvasService, title: &str) -> CreatedDesign {
    service
        .create(CreateDesign {
            title: title.into(),
            session_key: Some("main".into()),
            ..CreateDesign::default()
        })
        .await
        .unwrap()
}

fn context(reason: &str) -> CommitContext {
    CommitContext {
        reason: reason.into(),
        ..CommitContext::default()
    }
}

#[tokio::test]
async fn create_writes_entry_and_first_version() {
    let (_tmp, service, sink) = service_with(None).await;
    let created = create(&service, "Landing <page>").await;
    assert!(created.dir.starts_with(service.designs_root()));
    let html = std::fs::read_to_string(created.dir.join("index.html")).unwrap();
    assert!(html.contains("<title>Landing &lt;page&gt;</title>"));
    assert!(html.contains("data-mid=\"page\""));
    assert!(!html.contains("tokens.css"));
    assert_eq!(created.version.version, 1);
    assert_eq!(created.design.current_version, 1);
    assert_eq!(sink.names(), vec![
        "canvas.design.created",
        "canvas.version.created"
    ]);
}

#[tokio::test]
async fn create_with_design_system_links_tokens() {
    let (_tmp, service, _sink) = service_with(None).await;
    let created = service
        .create(CreateDesign {
            title: "x".into(),
            html: Some("<html><head><title>x</title></head><body></body></html>".into()),
            design_system: Some("midnight".into()),
            ..CreateDesign::default()
        })
        .await
        .unwrap();
    let html = std::fs::read_to_string(created.dir.join("index.html")).unwrap();
    assert!(html.starts_with("<html><head><link rel=\"stylesheet\" href=\"tokens.css\">"));
    assert!(
        std::fs::read_to_string(created.dir.join("tokens.css"))
            .unwrap()
            .contains("--color-accent")
    );
    assert_eq!(created.design.design_system.as_deref(), Some("midnight"));
    assert!(created.design_system.unwrap().guide.contains("Midnight"));
}

#[tokio::test]
async fn create_failures_leave_nothing_behind() {
    let (_tmp, service, _sink) = service_with(None).await;
    assert!(
        service
            .create(CreateDesign {
                title: "x".into(),
                design_system: Some("unknown".into()),
                ..CreateDesign::default()
            })
            .await
            .is_err()
    );
    assert!(
        service
            .create(CreateDesign {
                title: "  ".into(),
                ..CreateDesign::default()
            })
            .await
            .is_err()
    );
    assert!(
        service
            .create(CreateDesign {
                title: "big".into(),
                html: Some("x".repeat(11 * 1024 * 1024)),
                ..CreateDesign::default()
            })
            .await
            .is_err()
    );
    assert_eq!(
        std::fs::read_dir(service.designs_root()).unwrap().count(),
        0
    );
    assert!(
        service
            .list(&DesignFilter::default())
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn commit_skips_unchanged_content() {
    let (_tmp, service, sink) = service_with(None).await;
    let created = create(&service, "x").await;
    let id = &created.design.id;
    assert!(
        service
            .commit(id, &context("noop"))
            .await
            .unwrap()
            .is_none()
    );

    std::fs::write(created.dir.join("index.html"), "<p>v2</p>").unwrap();
    let v2 = service.commit(id, &context("edit")).await.unwrap().unwrap();
    assert_eq!(v2.version, 2);
    assert!(
        service
            .commit(id, &context("again"))
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(sink.names().len(), 3);
}

#[cfg(unix)]
#[tokio::test]
async fn commit_reports_symlinks_clearly() {
    let (tmp, service, _sink) = service_with(None).await;
    let created = create(&service, "x").await;
    std::fs::write(tmp.path().join("secret"), "s").unwrap();
    std::os::unix::fs::symlink(tmp.path().join("secret"), created.dir.join("leak")).unwrap();
    std::fs::write(created.dir.join("index.html"), "<p>edit</p>").unwrap();
    let err = service
        .commit(&created.design.id, &context("edit"))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("'leak'"), "{err}");
    std::fs::remove_file(created.dir.join("leak")).unwrap();
    std::fs::write(created.dir.join("index.html"), "<p>ok</p>").unwrap();
    assert!(
        service
            .commit(&created.design.id, &context("edit"))
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn revert_restores_files_as_a_new_version() {
    let (_tmp, service, sink) = service_with(None).await;
    let created = create(&service, "x").await;
    let id = &created.design.id;
    let index = created.dir.join("index.html");
    let original = std::fs::read_to_string(&index).unwrap();
    std::fs::write(&index, "<p>v2</p>").unwrap();
    std::fs::write(created.dir.join("extra.css"), "p{}").unwrap();
    service.commit(id, &context("edit")).await.unwrap();

    let reverted = service.revert(id, 1, Some("main".into())).await.unwrap();
    assert_eq!(reverted.version, 3);
    assert_eq!(reverted.reason, "revert to v1");
    assert_eq!(std::fs::read_to_string(&index).unwrap(), original);
    assert!(!created.dir.join("extra.css").exists());
    let history: Vec<_> = service
        .versions(id, 10)
        .await
        .unwrap()
        .into_iter()
        .map(|v| v.version)
        .collect();
    assert_eq!(history, vec![3, 2, 1]);
    {
        let events = sink.events.lock().unwrap();
        assert!(matches!(
            events.last(),
            Some(CanvasEvent::VersionCreated {
                reverted_from: Some(1),
                ..
            })
        ));
    }

    // Reverting to the content already current records nothing new.
    assert_eq!(service.revert(id, 3, None).await.unwrap().version, 3);
    assert!(service.revert(id, 42, None).await.is_err());
}

#[tokio::test]
async fn version_snapshots_are_immutable_and_served() {
    let (_tmp, service, _sink) = service_with(None).await;
    let created = create(&service, "x").await;
    let id = &created.design.id;
    std::fs::write(created.dir.join("index.html"), "<p>live</p>").unwrap();

    let v1 = service.source_dir(id, Some(1)).await.unwrap();
    assert!(
        std::fs::read_to_string(v1.join("index.html"))
            .unwrap()
            .contains("<!doctype html>")
    );
    let live = service.source_dir(id, None).await.unwrap();
    assert_eq!(
        std::fs::read_to_string(live.join("index.html")).unwrap(),
        "<p>live</p>"
    );
    assert!(service.source_dir(id, Some(9)).await.is_err());
    assert!(
        service
            .source_dir(&DesignId::generate(), None)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn preview_links_are_signed_and_scoped() {
    let (_tmp, service, _sink) = service_with(None).await;
    let created = create(&service, "x").await;
    let id = &created.design.id;
    let link = service.preview_link(id, None, None).await.unwrap();
    assert!(link.url.starts_with("http://127.0.0.1:18790/d/"));
    assert!(link.url.ends_with('/'));
    assert_eq!(link.sandbox, "allow-scripts");
    let token = link
        .url
        .trim_start_matches("http://127.0.0.1:18790/d/")
        .trim_end_matches('/');
    let claims = service.verify_token(token).unwrap();
    assert_eq!(&claims.design_id, id);
    assert_eq!(claims.version, None);

    let lan = service
        .preview_link(id, Some(1), Some("192.168.1.5:13131"))
        .await
        .unwrap();
    assert!(lan.url.starts_with("http://192.168.1.5:18790/d/"));
    assert_eq!(lan.version, Some(1));
    assert!(service.preview_link(id, Some(5), None).await.is_err());
    assert!(
        service
            .preview_link(&DesignId::generate(), None, None)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn public_url_overrides_origin() {
    let (tmp, _service, _sink) = service_with(None).await;
    let service = CanvasService::new(CanvasServiceParts {
        data_dir: tmp.path().to_path_buf(),
        pool: test_pool().await,
        settings: CanvasSettings::from(&antre_config::schema::CanvasConfig {
            public_url: Some("https://designs.example.com/".into()),
            script_sources: vec!["https://ok.example.com".into(), "bad".into()],
            ..Default::default()
        }),
        renderer: None,
        shares: None,
    })
    .unwrap();
    assert_eq!(
        service.origin(Some("evil.com")).as_deref(),
        Some("https://designs.example.com")
    );
    assert_eq!(service.settings.script_sources, vec![
        "https://ok.example.com"
    ]);
}

#[tokio::test]
async fn origin_is_unavailable_until_bound() {
    let tmp = tempfile::tempdir().unwrap();
    let service = CanvasService::new(CanvasServiceParts {
        data_dir: tmp.path().to_path_buf(),
        pool: test_pool().await,
        settings: CanvasSettings::default(),
        renderer: None,
        shares: None,
    })
    .unwrap();
    assert!(service.origin(None).is_none());
    service.set_render_address("0.0.0.0:4000".parse().unwrap());
    assert_eq!(
        service.origin(None).as_deref(),
        Some("http://127.0.0.1:4000")
    );
    assert_eq!(service.render_port(), Some(4000));
}

#[tokio::test]
async fn serve_file_respects_scope_and_size() {
    let (_tmp, service, _sink) = service_with(None).await;
    let created = create(&service, "x").await;
    std::fs::write(created.dir.join("big.bin"), vec![0_u8; 11 * 1024 * 1024]).unwrap();
    std::fs::write(created.dir.join("style.css"), "p{}").unwrap();
    let claims = TokenClaims {
        design_id: created.design.id.clone(),
        version: None,
        expires_at: i64::MAX,
    };
    let css = service.serve_file(&claims, "style.css").await.unwrap();
    assert_eq!(css.content_type, "text/css; charset=utf-8");
    assert!(!css.is_html);
    assert!(
        service
            .serve_file(&claims, "index.html")
            .await
            .unwrap()
            .is_html
    );
    assert!(matches!(
        service.serve_file(&claims, "big.bin").await.unwrap_err(),
        Error::TooLarge { .. }
    ));
    assert!(service.serve_file(&claims, "../../antre.db").await.is_err());
}

#[tokio::test]
async fn exports_in_every_format() {
    let renderer = Arc::new(FakeRenderer::default());
    let (_tmp, service, _sink) =
        service_with(Some(Arc::clone(&renderer) as Arc<dyn DesignRenderer>)).await;
    let created = create(&service, "My Deck").await;
    let id = &created.design.id;
    let viewport = Viewport::default();

    let html = service
        .export(id, None, ExportFormat::Html, viewport)
        .await
        .unwrap();
    assert_eq!(html.file_name, "my-deck.html");
    assert!(
        String::from_utf8(html.bytes)
            .unwrap()
            .contains("<title>My Deck</title>")
    );

    let zip = service
        .export(id, Some(1), ExportFormat::Zip, viewport)
        .await
        .unwrap();
    assert_eq!(zip.file_name, "my-deck-v1.zip");
    assert_eq!(zip.content_type, "application/zip");
    assert!(zip.bytes.starts_with(b"PK"));

    let png = service
        .export(id, None, ExportFormat::Png, viewport)
        .await
        .unwrap();
    assert_eq!(png.content_type, "image/png");
    let pdf = service
        .export(id, None, ExportFormat::Pdf, viewport)
        .await
        .unwrap();
    assert!(pdf.bytes.starts_with(b"%PDF"));
    assert_eq!(pdf.file_name, "my-deck.pdf");

    let requests = renderer.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(
        requests
            .iter()
            .all(|r| r.html.contains("Content-Security-Policy"))
    );
    assert_eq!(requests[1].format, RenderFormat::Pdf);
}

#[tokio::test]
async fn render_without_browser_is_unavailable() {
    let tmp = tempfile::tempdir().unwrap();
    let service = CanvasService::new(CanvasServiceParts {
        data_dir: tmp.path().to_path_buf(),
        pool: test_pool().await,
        settings: CanvasSettings::default(),
        renderer: None,
        shares: None,
    })
    .unwrap();
    let created = create(&service, "x").await;
    let err = service
        .render(
            &created.design.id,
            None,
            RenderFormat::Png,
            Viewport::default(),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Unavailable(_)));
    assert!(matches!(
        service
            .publish(&created.design.id, None, Visibility::Public, None, None)
            .await
            .unwrap_err(),
        Error::Unavailable(_)
    ));
}

#[tokio::test]
async fn publish_freezes_a_version_and_can_be_revoked() {
    let (_tmp, service, sink) = service_with(None).await;
    let created = create(&service, "Pitch").await;
    let id = &created.design.id;
    std::fs::write(created.dir.join("index.html"), "<p>unsaved</p>").unwrap();

    let published = service
        .publish(id, None, Visibility::Private, None, None)
        .await
        .unwrap();
    assert_eq!(
        published.version, 2,
        "live edits are versioned before publishing"
    );
    let key = published.access_key.clone().unwrap();
    assert!(
        published
            .url
            .ends_with(&format!("/p/{}?k={key}", published.share_id))
    );

    assert!(
        service
            .published(&published.share_id, None)
            .await
            .unwrap()
            .is_none()
    );
    let snapshot = service
        .published(&published.share_id, Some(&key))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(snapshot.html, "<p>unsaved</p>");
    assert_eq!(snapshot.title, "Pitch");

    std::fs::write(created.dir.join("index.html"), "<p>later</p>").unwrap();
    let frozen = service
        .published(&published.share_id, Some(&key))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        frozen.html, "<p>unsaved</p>",
        "published snapshot is immutable"
    );

    assert!(service.unpublish(&published.share_id).await.unwrap());
    assert!(
        service
            .published(&published.share_id, Some(&key))
            .await
            .unwrap()
            .is_none()
    );
    assert!(service.unpublish("not-a-uuid").await.is_err());
    assert!(
        !service
            .unpublish("67e55044-10b1-426f-9247-bb680e5fe0c8")
            .await
            .unwrap()
    );
    assert!(sink.names().contains(&"canvas.design.published"));
    assert!(sink.names().contains(&"canvas.design.unpublished"));

    let public = service
        .publish(id, Some(1), Visibility::Public, None, None)
        .await
        .unwrap();
    assert!(public.access_key.is_none());
    assert_eq!(public.version, 1);
    assert!(
        service
            .published(&public.share_id, None)
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn turn_activity_tracks_runs() {
    let (_tmp, service, _sink) = service_with(None).await;
    let first = DesignId::generate();
    let second = DesignId::generate();
    assert!(service.touch("s", Some("r1"), &first).is_none());
    assert!(service.touch("s", Some("r1"), &second).is_none());
    let flushed = service.touch("s", Some("r2"), &first).unwrap();
    assert_eq!(flushed.run_id.as_deref(), Some("r1"));
    assert_eq!(flushed.designs.len(), 2);
    let ended = service.end_turn("s").unwrap();
    assert_eq!(ended.designs, vec![first]);
    assert!(service.end_turn("s").is_none());
}

#[tokio::test]
async fn info_lists_vendor_and_design_systems() {
    let (_tmp, service, _sink) = service_with(None).await;
    let info = service.info(None).await;
    assert_eq!(info.origin.as_deref(), Some("http://127.0.0.1:18790"));
    assert_eq!(info.bridge_protocol, BRIDGE_PROTOCOL);
    assert_eq!(info.design_systems.len(), 3);
    assert!(info.vendor.iter().any(|entry| entry.name == "preact.js"));
}

#[test]
fn export_format_parses() {
    assert_eq!("pdf".parse::<ExportFormat>().unwrap(), ExportFormat::Pdf);
    assert!("exe".parse::<ExportFormat>().is_err());
    assert_eq!(ExportFormat::Zip.label(), "zip");
}

#[test]
fn ensure_tokens_link_handles_documents_without_head() {
    assert_eq!(
        ensure_tokens_link("<p>x</p>"),
        "<link rel=\"stylesheet\" href=\"tokens.css\"><p>x</p>"
    );
    let linked = "<link href=\"tokens.css\">";
    assert_eq!(ensure_tokens_link(linked), linked);
}

#[tokio::test]
async fn comments_pin_to_versions_and_emit_events() {
    let (_tmp, service, sink) = service_with(None).await;
    let created = create(&service, "Pins").await;
    let id = &created.design.id;
    let note = |body: &str| crate::NewComment {
        mid: Some("hero".into()),
        body: body.into(),
        ..crate::NewComment::default()
    };
    let on_current = service.add_comment(id, None, note("bigger")).await.unwrap();
    assert_eq!(on_current.version, 1);
    assert!(service.add_comment(id, Some(9), note("x")).await.is_err());
    assert_eq!(service.comments(id, Some(1), false).await.unwrap().len(), 1);
    assert_eq!(
        service
            .resolve_comments(id, std::slice::from_ref(&on_current.id))
            .await
            .unwrap(),
        1
    );
    assert!(service.comments(id, None, false).await.unwrap().is_empty());
    assert!(service.delete_comment(id, &on_current.id).await.unwrap());
    assert!(!service.delete_comment(id, &on_current.id).await.unwrap());
    let changes = sink
        .names()
        .into_iter()
        .filter(|name| *name == "canvas.comments.changed")
        .count();
    assert_eq!(changes, 3);
    let missing = DesignId::generate();
    assert!(
        service
            .comments(&missing, None, false)
            .await
            .unwrap_err()
            .is_not_found()
    );
}
