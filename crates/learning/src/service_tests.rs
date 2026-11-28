#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Mutex as StdMutex;

use {antre_config::LearningConfig, async_trait::async_trait};

use super::*;

struct CannedModel {
    answer: std::result::Result<String, String>,
    requests: StdMutex<Vec<ReviewRequest>>,
}

impl CannedModel {
    fn new(answer: &str) -> Self {
        Self {
            answer: Ok(answer.to_string()),
            requests: StdMutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl ReviewModel for CannedModel {
    fn id(&self) -> &str {
        "test::cheap"
    }

    async fn complete(&self, request: &ReviewRequest) -> std::result::Result<String, String> {
        self.requests
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(request.clone());
        self.answer.clone()
    }
}

fn config(write_approval: bool) -> LearningConfig {
    let mut config = LearningConfig {
        enabled: true,
        write_approval,
        ..LearningConfig::default()
    };
    config.review.every_user_turns = 2;
    config
}

fn service(write_approval: bool) -> (tempfile::TempDir, LearningService) {
    let tmp = tempfile::tempdir().unwrap();
    let service = LearningService::new(
        tmp.path(),
        config(write_approval),
        AgentMemoryWriteMode::Hybrid,
    );
    (tmp, service)
}

fn write_skill(service: &LearningService, name: &str, body: &str) {
    let dir = service.paths().skills_dir().join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {name} procedure\n---\n{body}\n"),
    )
    .unwrap();
}

fn conversation() -> Vec<Turn> {
    vec![
        Turn::user("cut the release"),
        Turn::assistant("merged main into the branch", vec![
            "read_skill".into(),
            "exec".into(),
        ]),
        Turn::user("never merge, always rebase"),
        Turn::assistant("rebased", vec!["exec".into()]),
    ]
}

const ANSWER: &str = r#"{"changes": [
  {"target": {"kind": "skill", "name": "release-process", "file": "SKILL.md"}, "op": "replace", "old": "Update the branch.", "new": "Update the branch with `git rebase origin/main`, never a merge: the history must stay linear.", "reason": "user corrected the workflow"},
  {"target": {"kind": "memory", "file": "MEMORY.md"}, "op": "append", "text": "- Releases are cut from main.", "reason": "environment fact"},
  {"target": {"kind": "memory", "file": "MEMORY.md"}, "op": "append", "text": "- The CI runner is broken.", "reason": "bad"}
]}"#;

#[tokio::test]
async fn review_stages_guarded_changes_then_approval_applies_them() {
    let (_tmp, service) = service(true);
    write_skill(
        &service,
        "release-process",
        "1. Update the branch.\n2. Tag.",
    );
    let model = CannedModel::new(ANSWER);
    let turns = conversation();
    assert!(service.review_due("main", &turns).await);

    let loaded = HashSet::from(["release-process".to_string()]);
    let report = service
        .run_review(&model, "main", &turns, &loaded, &HashSet::new(), 20_000)
        .await
        .unwrap();
    let kinds: Vec<&str> = report.outcomes.iter().map(Outcome::kind).collect();
    assert_eq!(kinds, ["staged", "staged", "refused"]);
    assert!(!service.review_due("main", &turns).await);

    let request = model.requests.lock().unwrap()[0].clone();
    assert!(request.user.contains("# Loaded skill: release-process"));
    assert!(request.user.contains("never merge, always rebase"));
    assert!(request.system.contains("Never capture"));

    let pending = service.pending().await.unwrap();
    assert_eq!(pending.len(), 2);
    assert!(
        pending
            .iter()
            .all(|view| !view.stale && view.problem.is_none())
    );
    assert!(
        pending[0]
            .diff
            .contains("+1. Update the branch with `git rebase")
    );

    let skill_id = pending[0].proposal.id.clone();
    let memory_id = pending[1].proposal.id.clone();
    let entry = service.approve(&skill_id).await.unwrap();
    assert!(entry.approved);
    assert_eq!(entry.actor, Actor::Review);
    service
        .reject(&memory_id, Some("not useful".into()))
        .await
        .unwrap();
    assert!(service.pending().await.unwrap().is_empty());
    let skill_md = std::fs::read_to_string(
        service
            .paths()
            .skills_dir()
            .join("release-process/SKILL.md"),
    )
    .unwrap();
    assert!(skill_md.contains("never a merge"));

    // The rejection is a hint for the next review.
    let next = service.review_request(&turns, &loaded, 10_000).await;
    assert!(next.user.contains("Releases are cut from main"));

    // Roll the approved edit back.
    let ledger = service.ledger(None, 10).await.unwrap();
    assert_eq!(ledger.len(), 1);
    assert!(
        service
            .ledger_diff(&entry.id)
            .await
            .unwrap()
            .contains("never a merge")
    );
    service
        .rollback(&entry.id, "prefer my wording")
        .await
        .unwrap();
    let skill_md = std::fs::read_to_string(
        service
            .paths()
            .skills_dir()
            .join("release-process/SKILL.md"),
    )
    .unwrap();
    assert!(skill_md.contains("1. Update the branch.\n"));
    let ledger = service.ledger(Some("release-process"), 10).await.unwrap();
    assert_eq!(ledger.len(), 2);
    assert!(ledger[1].reverted_by.is_some());
}

#[tokio::test]
async fn without_write_approval_changes_apply_directly() {
    let (_tmp, service) = service(false);
    write_skill(&service, "release-process", "1. Update the branch.");
    let model = CannedModel::new(ANSWER);
    let report = service
        .run_review(
            &model,
            "main",
            &conversation(),
            &HashSet::new(),
            &HashSet::new(),
            20_000,
        )
        .await
        .unwrap();
    let kinds: Vec<&str> = report.outcomes.iter().map(Outcome::kind).collect();
    assert_eq!(kinds, ["applied", "applied", "refused"]);
    let memory = std::fs::read_to_string(service.paths().data_dir().join("MEMORY.md")).unwrap();
    assert_eq!(memory, "- Releases are cut from main.\n");
    assert_eq!(service.status().await.unwrap().ledger_entries, 2);
}

#[tokio::test]
async fn approval_refuses_stale_and_locked_targets() {
    let (_tmp, service) = service(true);
    write_skill(&service, "release-process", "1. Update the branch.");
    let model = CannedModel::new(ANSWER);
    service
        .run_review(
            &model,
            "main",
            &conversation(),
            &HashSet::new(),
            &HashSet::new(),
            20_000,
        )
        .await
        .unwrap();
    let pending = service.pending().await.unwrap();
    std::fs::write(service.paths().data_dir().join("MEMORY.md"), "hand edit\n").unwrap();
    let memory = pending
        .iter()
        .find(|view| view.proposal.target == Target::memory("MEMORY.md"))
        .unwrap();
    assert!(
        service
            .pending_get(&memory.proposal.id)
            .await
            .unwrap()
            .stale
    );
    assert!(matches!(
        service.approve(&memory.proposal.id).await,
        Err(Error::StaleTarget(_))
    ));

    service
        .set_flags("release-process", None, Some(true))
        .await
        .unwrap();
    let skill = pending
        .iter()
        .find(|view| view.proposal.target.skill_name().is_some())
        .unwrap();
    assert!(matches!(
        service.approve(&skill.proposal.id).await,
        Err(Error::Refused(_))
    ));
    assert_eq!(service.status().await.unwrap().locked_skills, [
        "release-process"
    ]);
    assert!(
        service
            .set_flags("Bad Name", Some(true), None)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn memory_write_mode_and_toggles_limit_targets() {
    let tmp = tempfile::tempdir().unwrap();
    let mut config = config(false);
    config.review.skills = false;
    let service = LearningService::new(tmp.path(), config, AgentMemoryWriteMode::SearchOnly);
    write_skill(&service, "release-process", "1. Update the branch.");
    let answer = r#"{"changes": [
      {"target": {"kind": "skill", "name": "release-process"}, "op": "append", "text": "3. Push.", "reason": "x"},
      {"target": {"kind": "memory", "file": "MEMORY.md"}, "op": "append", "text": "- a", "reason": "x"},
      {"target": {"kind": "memory", "file": "memory/user-preferences.md"}, "op": "append", "text": "- Prefers short answers.", "reason": "x"}
    ]}"#;
    let report = service
        .run_review(
            &CannedModel::new(answer),
            "main",
            &conversation(),
            &HashSet::new(),
            &HashSet::new(),
            20_000,
        )
        .await
        .unwrap();
    let kinds: Vec<&str> = report.outcomes.iter().map(Outcome::kind).collect();
    assert_eq!(kinds, ["refused", "refused", "applied"]);
}

#[tokio::test]
async fn max_changes_and_model_errors() {
    let (_tmp, service) = service(true);
    let mut many = String::from(r#"{"changes": ["#);
    for index in 0..6 {
        if index > 0 {
            many.push(',');
        }
        many.push_str(&format!(
            r#"{{"target": {{"kind": "memory", "file": "MEMORY.md"}}, "op": "append", "text": "- fact {}", "reason": "x"}}"#,
            ["one", "two", "three", "four", "five", "six"][index]
        ));
    }
    many.push_str("]}");
    let report = service
        .run_review(
            &CannedModel::new(&many),
            "main",
            &conversation(),
            &HashSet::new(),
            &HashSet::new(),
            20_000,
        )
        .await
        .unwrap();
    assert_eq!(report.outcomes.len(), 3);

    let failing = CannedModel {
        answer: Err("rate limited".into()),
        requests: StdMutex::new(Vec::new()),
    };
    let error = service
        .run_review(
            &failing,
            "other",
            &conversation(),
            &HashSet::new(),
            &HashSet::new(),
            20_000,
        )
        .await
        .unwrap_err();
    assert!(matches!(error, Error::Model(_)));
    // A failed review does not mark the session reviewed.
    assert!(service.review_due("other", &conversation()).await);
}

#[tokio::test]
async fn disabled_loop_never_reviews_or_curates() {
    let tmp = tempfile::tempdir().unwrap();
    let service = LearningService::new(
        tmp.path(),
        LearningConfig::default(),
        AgentMemoryWriteMode::Hybrid,
    );
    assert!(!service.review_due("main", &conversation()).await);
    assert!(
        service
            .curator_tick(&[], Duration::hours(10), &HashSet::new())
            .await
            .unwrap()
            .is_none()
    );
    assert!(!service.status().await.unwrap().enabled);
}

fn activity(name: &str, days_ago: i64) -> SkillActivity {
    SkillActivity {
        name: name.into(),
        last_used: Some(OffsetDateTime::now_utc() - Duration::days(days_ago)),
        created: None,
        uses: 3,
    }
}

#[tokio::test]
async fn curator_seeds_then_archives_and_rollback_restores() {
    let (_tmp, service) = service(false);
    write_skill(&service, "old-habit", "steps");
    write_skill(&service, "daily-flow", "steps");
    let skills = [activity("old-habit", 90), activity("daily-flow", 1)];

    // First tick seeds the schedule; the next one waits for the interval.
    assert!(
        service
            .curator_tick(&skills, Duration::hours(10), &HashSet::new())
            .await
            .unwrap()
            .is_none()
    );
    assert!(service.status().await.unwrap().last_curator_run.is_some());
    assert!(
        service
            .curator_tick(&skills, Duration::hours(10), &HashSet::new())
            .await
            .unwrap()
            .is_none()
    );

    // A forced run first sees the skills (seeding their clocks)...
    let first = service
        .run_curator(&skills, &HashSet::new(), false)
        .await
        .unwrap();
    assert_eq!(first.plan.seeded.len(), 2);
    assert!(first.outcomes.is_empty());

    // ...and archives once the clock has run, backdated here by hand.
    {
        let mut inner = service.inner.lock().await;
        for record in inner.curator.skills.values_mut() {
            record.first_seen -= Duration::days(100);
        }
    }
    let dry = service
        .run_curator(&skills, &HashSet::new(), true)
        .await
        .unwrap();
    assert_eq!(dry.plan.archive, ["old-habit"]);
    assert!(service.paths().skills_dir().join("old-habit").exists());

    let report = service
        .run_curator(&skills, &HashSet::new(), false)
        .await
        .unwrap();
    let Outcome::Applied { entry } = &report.outcomes[0] else {
        panic!("expected an applied archive, got {:?}", report.outcomes);
    };
    assert_eq!(entry.actor, Actor::Curator);
    assert!(!service.paths().skills_dir().join("old-habit").exists());
    assert_eq!(service.personal_skill_names(), ["daily-flow"]);

    service.rollback(&entry.id, "still needed").await.unwrap();
    assert!(
        service
            .paths()
            .skills_dir()
            .join("old-habit/SKILL.md")
            .exists()
    );
}

#[tokio::test]
async fn pinned_skills_are_never_archived() {
    let (_tmp, service) = service(true);
    write_skill(&service, "rare-but-vital", "steps");
    service
        .set_flags("rare-but-vital", Some(true), None)
        .await
        .unwrap();
    {
        let mut inner = service.inner.lock().await;
        inner
            .curator
            .skills
            .get_mut("rare-but-vital")
            .unwrap()
            .first_seen -= Duration::days(365);
    }
    let report = service
        .run_curator(&[activity("rare-but-vital", 365)], &HashSet::new(), false)
        .await
        .unwrap();
    assert!(report.plan.archive.is_empty());
    assert_eq!(service.status().await.unwrap().pinned_skills, [
        "rare-but-vital"
    ]);
}

#[tokio::test]
async fn consolidation_stages_a_merge_group() {
    let (_tmp, service) = service(true);
    write_skill(&service, "git-rebase", "Rebase onto main.");
    write_skill(&service, "git-push", "Push with -u.");
    let answer = r#"{"merges": [{"into": "git-workflow", "absorb": ["git-rebase", "git-push"], "skill_md": "---\nname: git-workflow\ndescription: Branch updates and pushes with git\n---\n1. Rebase onto main.\n2. Push with -u.\n", "reason": "same class of task"}]}"#;
    let model = CannedModel::new(answer);
    let outcomes = service
        .run_consolidation(&model, &HashSet::new())
        .await
        .unwrap();
    let kinds: Vec<&str> = outcomes.iter().map(Outcome::kind).collect();
    assert_eq!(kinds, ["staged", "staged", "staged"]);
    let pending = service.pending().await.unwrap();
    assert!(
        pending
            .iter()
            .any(|view| view.diff.starts_with("archive skills/git-push"))
    );
    for view in pending {
        service.approve(&view.proposal.id).await.unwrap();
    }
    assert_eq!(service.personal_skill_names(), ["git-workflow"]);
    assert!(service.skill_sha("git-workflow").unwrap().is_some());
}

#[tokio::test]
async fn consolidation_needs_two_candidates() {
    let (_tmp, service) = service(true);
    write_skill(&service, "only-one", "x");
    let model = CannedModel::new("{}");
    assert!(
        service
            .run_consolidation(&model, &HashSet::new())
            .await
            .unwrap()
            .is_empty()
    );
    assert!(model.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn state_survives_a_restart() {
    let tmp = tempfile::tempdir().unwrap();
    {
        let service = LearningService::new(tmp.path(), config(true), AgentMemoryWriteMode::Hybrid);
        service
            .set_flags("kept", Some(true), Some(true))
            .await
            .unwrap();
        service
            .run_review(
                &CannedModel::new(r#"{"changes": []}"#),
                "main",
                &conversation(),
                &HashSet::new(),
                &HashSet::new(),
                1_000,
            )
            .await
            .unwrap();
    }
    let service = LearningService::new(tmp.path(), config(true), AgentMemoryWriteMode::Hybrid);
    let status = service.status().await.unwrap();
    assert_eq!(status.locked_skills, ["kept"]);
    assert!(!service.review_due("main", &conversation()).await);
    std::fs::write(service.paths().curator_state(), "{broken").unwrap();
    let fresh = LearningService::new(tmp.path(), config(true), AgentMemoryWriteMode::Hybrid);
    assert!(fresh.status().await.unwrap().locked_skills.is_empty());
}
