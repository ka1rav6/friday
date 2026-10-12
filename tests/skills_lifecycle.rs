//! Skill lifecycle acceptance criteria: add from a template and from files,
//! reject malformed packages, inspect declared access, enable/disable, run,
//! update without clobbering local edits, and remove with a keep/delete choice.
use friday::models::MockProvider;
use friday::skills;
use serde_json::json;
use std::path::Path;

async fn state_with_skill_dirs() -> (
    friday::state::AppState,
    std::sync::Arc<MockProvider>,
    tempfile::TempDir,
) {
    let (st, mock) = friday::test_state().await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    {
        let mut cfg = st.config.write().await;
        cfg.skills.user_dir = tmp.path().join("user");
        cfg.skills.project_dir = tmp.path().join("project");
    }
    std::fs::create_dir_all(tmp.path().join("user")).unwrap();
    std::fs::create_dir_all(tmp.path().join("project")).unwrap();
    (st, mock, tmp)
}

fn write_pkg(dir: &Path, manifest: &str, md: &str) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join("skill.toml"), manifest).unwrap();
    std::fs::write(dir.join("SKILL.md"), md).unwrap();
}

/// Criterion 1: a GUI/CLI-created skill and a hand-copied one land in the same library.
#[tokio::test]
async fn template_created_and_hand_copied_skills_share_one_library() {
    let (st, _m, tmp) = state_with_skill_dirs().await;

    let made = skills::create_from_template(&st, "weekly-review", "Summarize the week.", "project")
        .await
        .unwrap();
    assert_eq!(made["action"], json!("installed"));
    assert_eq!(
        made["enabled"],
        json!(false),
        "a new skill must not be active until enabled"
    );

    // A second skill arrives by copying files into the scope directory.
    write_pkg(
        &tmp.path().join("project/hand-copied"),
        "name = \"hand-copied\"\nversion = \"0.2.0\"\ndescription = \"Dropped in by hand.\"\ndeclared_tools = [\"search_knowledge\"]\n",
        "# hand-copied\n\n## Steps\n1. Search saved knowledge.\n",
    );
    let scan = skills::rescan(&st).await.unwrap();
    assert!(scan["rejected"].as_array().unwrap().is_empty(), "{scan}");

    let names: Vec<String> = skills::list(&st)
        .await
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap().to_string())
        .collect();
    assert!(names.contains(&"weekly-review".to_string()));
    assert!(names.contains(&"hand-copied".to_string()));
    // The built-in briefing skill is in the same list.
    friday::scheduler::register_builtins(&st).await.unwrap();
    let all = skills::list(&st).await.unwrap();
    assert!(all
        .iter()
        .any(|s| s["name"] == "morning-briefing" && s["scope"] == "builtin"));
}

/// Criterion 2: malformed metadata is rejected with a specific, actionable error.
#[tokio::test]
async fn malformed_packages_are_rejected_with_specific_errors() {
    let (st, _m, tmp) = state_with_skill_dirs().await;
    let base = tmp.path();

    write_pkg(&base.join("no-name"), "version = \"1\"\n", "x");
    let e = format!(
        "{:#}",
        skills::install(&st, &base.join("no-name"), "project", false)
            .await
            .unwrap_err()
    );
    assert!(e.contains("invalid manifest"), "{e}");

    write_pkg(
        &base.join("bad-name"),
        "name = \"Not Kebab\"\nversion = \"1\"\n",
        "x",
    );
    let e = format!(
        "{:#}",
        skills::install(&st, &base.join("bad-name"), "project", false)
            .await
            .unwrap_err()
    );
    assert!(e.contains("lowercase letters, digits"), "{e}");

    write_pkg(&base.join("no-version"), "name = \"ok\"\n", "x");
    let e = format!(
        "{:#}",
        skills::install(&st, &base.join("no-version"), "project", false)
            .await
            .unwrap_err()
    );
    assert!(e.contains("version"), "{e}");

    std::fs::create_dir_all(base.join("no-md")).unwrap();
    std::fs::write(
        base.join("no-md/skill.toml"),
        "name = \"m\"\nversion = \"1\"\n",
    )
    .unwrap();
    let e = format!(
        "{:#}",
        skills::install(&st, &base.join("no-md"), "project", false)
            .await
            .unwrap_err()
    );
    assert!(e.contains("SKILL.md"), "{e}");

    write_pkg(
        &base.join("unknown-key"),
        "name = \"u\"\nversion = \"1\"\nsudo = true\n",
        "x",
    );
    assert!(
        skills::install(&st, &base.join("unknown-key"), "project", false)
            .await
            .is_err()
    );

    // Nothing malformed was registered.
    assert!(skills::list(&st).await.unwrap().is_empty());
}

#[tokio::test]
async fn a_symlink_escaping_the_package_is_refused() {
    let (st, _m, tmp) = state_with_skill_dirs().await;
    let d = tmp.path().join("leaky");
    write_pkg(&d, "name = \"leaky\"\nversion = \"1\"\n", "# leaky\n");
    let outside = tmp.path().join("private.txt");
    std::fs::write(&outside, "secret").unwrap();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&outside, d.join("leak.txt")).unwrap();
        let e = format!(
            "{:#}",
            skills::install(&st, &d, "project", false)
                .await
                .unwrap_err()
        );
        assert!(e.contains("symlink"), "{e}");
    }
}

/// Criterion 3: requested capabilities are shown before enabling, and a skill
/// cannot exceed the user's permissions.
#[tokio::test]
async fn inspect_shows_declared_access_against_current_policy() {
    let (st, _m, tmp) = state_with_skill_dirs().await;
    st.config.write().await.permissions.allow_network = false;

    let d = tmp.path().join("needs-net");
    write_pkg(
        &d,
        "name = \"needs-net\"\nversion = \"1\"\ndeclared_tools = [\"search_web\", \"search_knowledge\", \"no_such_tool\"]\n",
        "# needs-net\n",
    );
    let report = skills::inspect(&st, &d).await.unwrap();
    let declared = &report["declared_tools"];
    assert!(declared["allowed_by_policy"]
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t["tool"] == "search_knowledge"));
    assert!(declared["blocked_by_policy"]
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t["tool"] == "search_web"));
    assert_eq!(declared["unknown"], json!(["no_such_tool"]));
    assert_eq!(report["scripts_will_run"], json!(false));
    assert!(report["content_hash"].as_str().unwrap().len() >= 8);

    // Declaring a tool does not grant it: the policy still refuses at run time.
    skills::install(&st, &d, "project", true).await.unwrap();
    let ctx = friday::tools::ToolCtx::root(st.clone(), None);
    let denied = friday::tools::execute(&ctx, "search_web", json!({"query":"x"})).await;
    assert!(!denied.ok);
    assert!(denied.value["error"]
        .as_str()
        .unwrap()
        .contains("allow_network"));
}

#[tokio::test]
async fn scripts_are_detected_and_never_executed_on_install() {
    let (st, _m, tmp) = state_with_skill_dirs().await;
    let d = tmp.path().join("scripted");
    write_pkg(&d, "name = \"scripted\"\nversion = \"1\"\n", "# scripted\n");
    std::fs::create_dir_all(d.join("scripts")).unwrap();
    let marker = tmp.path().join("pwned");
    std::fs::write(
        d.join("scripts/install.sh"),
        format!("#!/bin/sh\ntouch {}\n", marker.display()),
    )
    .unwrap();

    let out = skills::install(&st, &d, "project", false).await.unwrap();
    assert_eq!(out["scripts_detected"].as_array().unwrap().len(), 1);
    assert!(!marker.exists(), "an install hook was executed");
}

/// Criteria 4 and 6: disable/enable loses nothing, and an update preserves the
/// enabled state and the on-disk edits.
#[tokio::test]
async fn disable_enable_and_update_preserve_state_and_local_edits() {
    let (st, mock, tmp) = state_with_skill_dirs().await;
    let d = tmp.path().join("project/versioned");
    write_pkg(
        &d,
        "name = \"versioned\"\nversion = \"1.0.0\"\ndescription = \"v1\"\ndeclared_tools = [\"search_knowledge\"]\n",
        "# versioned\n\n## Steps\n1. Original step.\n",
    );
    skills::install(&st, &d, "project", true).await.unwrap();
    assert!(skills::is_runnable(&st, "versioned").await.unwrap());

    // Disabling it takes it out of circulation without deleting anything.
    skills::set_enabled(&st, "versioned", false).await.unwrap();
    assert!(!skills::is_runnable(&st, "versioned").await.unwrap());
    assert!(!skills::prompt_block(&st)
        .await
        .unwrap()
        .contains("versioned"));
    mock.push(MockProvider::text("should not run"));
    let e = format!(
        "{:#}",
        skills::run(&st, "versioned", None, None).await.unwrap_err()
    );
    assert!(e.contains("disabled"), "{e}");
    assert!(
        d.join("SKILL.md").exists(),
        "disabling must not delete files"
    );

    // Re-enabling restores it with its configuration intact.
    skills::set_enabled(&st, "versioned", true).await.unwrap();
    assert!(skills::is_runnable(&st, "versioned").await.unwrap());

    // A local edit plus a version bump: the update reports the diff and keeps
    // the enabled state. The file on disk is the source of truth.
    std::fs::write(
        d.join("skill.toml"),
        "name = \"versioned\"\nversion = \"1.1.0\"\ndescription = \"v2\"\ndeclared_tools = [\"search_knowledge\"]\n",
    )
    .unwrap();
    std::fs::write(
        d.join("SKILL.md"),
        "# versioned\n\n## Steps\n1. Locally edited step.\n",
    )
    .unwrap();
    let upd = skills::install(&st, &d, "project", false).await.unwrap();
    assert_eq!(upd["action"], json!("updated"));
    assert_eq!(upd["version"]["from"], json!("1.0.0"));
    assert_eq!(upd["version"]["to"], json!("1.1.0"));
    assert_eq!(upd["content_changed"], json!(true));
    assert_eq!(
        upd["enabled"],
        json!(true),
        "an update must not silently disable a skill"
    );
    assert!(std::fs::read_to_string(d.join("SKILL.md"))
        .unwrap()
        .contains("Locally edited step"));
}

/// Criterion 5: removal honours the keep-or-delete choice.
#[tokio::test]
async fn removal_keeps_files_by_default_and_deletes_only_on_request() {
    let (st, _m, tmp) = state_with_skill_dirs().await;

    let keep = tmp.path().join("project/keeper");
    write_pkg(&keep, "name = \"keeper\"\nversion = \"1\"\n", "# keeper\n");
    skills::install(&st, &keep, "project", true).await.unwrap();
    let out = skills::remove(&st, "keeper", false).await.unwrap();
    assert_eq!(out["files_deleted"], json!(false));
    assert!(
        keep.exists(),
        "files must be kept unless deletion is requested"
    );
    assert!(skills::list(&st)
        .await
        .unwrap()
        .iter()
        .all(|s| s["name"] != "keeper"));

    let gone = tmp.path().join("project/goner");
    write_pkg(&gone, "name = \"goner\"\nversion = \"1\"\n", "# goner\n");
    skills::install(&st, &gone, "project", true).await.unwrap();
    let out = skills::remove(&st, "goner", true).await.unwrap();
    assert_eq!(out["files_deleted"], json!(true));
    assert!(!gone.exists());

    assert!(skills::remove(&st, "never-existed", false).await.is_err());
}

#[tokio::test]
async fn deletion_is_refused_for_packages_outside_the_managed_directories() {
    let (st, _m, tmp) = state_with_skill_dirs().await;
    let elsewhere = tmp.path().join("somewhere-else/external");
    write_pkg(
        &elsewhere,
        "name = \"external\"\nversion = \"1\"\n",
        "# external\n",
    );
    skills::install(&st, &elsewhere, "project", true)
        .await
        .unwrap();
    let e = format!(
        "{:#}",
        skills::remove(&st, "external", true).await.unwrap_err()
    );
    assert!(e.contains("outside the managed skill"), "{e}");
    assert!(
        elsewhere.exists(),
        "an out-of-tree package must not be deleted"
    );
}

#[tokio::test]
async fn running_a_skill_records_its_run_history() {
    let (st, mock, tmp) = state_with_skill_dirs().await;
    let d = tmp.path().join("project/runnable");
    write_pkg(
        &d,
        "name = \"runnable\"\nversion = \"1\"\ndescription = \"Does a thing.\"\ndeclared_tools = [\"get_current_time\"]\n",
        "# runnable\n\n## Steps\n1. Check the time.\n",
    );
    skills::install(&st, &d, "project", true).await.unwrap();

    mock.push(MockProvider::call("get_current_time", json!({})));
    mock.push(MockProvider::text("The skill completed."));
    let out = skills::run(&st, "runnable", Some("go"), None)
        .await
        .unwrap();
    assert_eq!(out["status"], json!("completed"));
    assert!(out["output"].as_str().unwrap().contains("completed"));

    let history = skills::run_history(&st, Some("runnable")).await.unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0]["status"], json!("completed"));

    // The skill ran with its declared grant, not with everything.
    let granted: String =
        sqlx::query_scalar("SELECT allowed_tools FROM agent_runs WHERE task_id=?")
            .bind(out["task_id"].as_str().unwrap())
            .fetch_one(&st.db)
            .await
            .unwrap();
    let granted: Vec<String> = serde_json::from_str(&granted).unwrap();
    assert!(granted.contains(&"get_current_time".to_string()));
    assert!(!granted.contains(&"delete_memory".to_string()));
}

#[tokio::test]
async fn a_draft_generated_from_a_trace_is_installed_disabled() {
    let (st, mock, _tmp) = state_with_skill_dirs().await;
    mock.push(MockProvider::call("get_current_time", json!({})));
    mock.push(MockProvider::text("done"));
    let run = friday::agents::handle_message(&st, "c", "what time is it")
        .await
        .unwrap();

    let draft = skills::draft_from_trace(&st, "time-check", "Check the time.", &run.task_id)
        .await
        .unwrap();
    assert_eq!(
        draft["enabled"],
        json!(false),
        "a generated skill must never auto-activate"
    );
    assert_eq!(draft["declared_tools"], json!(["get_current_time"]));
    let md = std::fs::read_to_string(Path::new(draft["path"].as_str().unwrap()).join("SKILL.md"))
        .unwrap();
    assert!(md.contains("Review checklist"));
    assert!(!skills::is_runnable(&st, "time-check").await.unwrap());
}

#[tokio::test]
async fn only_registered_enabled_skills_can_be_scheduled() {
    let (st, _m, _tmp) = state_with_skill_dirs().await;
    let ctx = friday::tools::ToolCtx::root(st.clone(), None);

    let out = friday::tools::execute(
        &ctx,
        "schedule_job",
        json!({"name":"evil","skill":"rm -rf /","at_time":"03:00"}),
    )
    .await;
    assert!(!out.ok);
    assert!(out.value["error"]
        .as_str()
        .unwrap()
        .contains("not registered and enabled"));

    friday::scheduler::register_builtins(&st).await.unwrap();
    let ok = friday::tools::execute(
        &ctx, "schedule_job",
        json!({"name":"daily-brief","skill":"morning-briefing","at_time":"07:30","timezone":"Asia/Kolkata"}),
    )
    .await;
    assert!(ok.ok, "{:?}", ok.value);
    assert!(ok.value["next_run_at"].is_string());
    assert!(ok.value["caveat"]
        .as_str()
        .unwrap()
        .contains("only fires while"));
}
