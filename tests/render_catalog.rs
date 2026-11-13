use rustifiedgit::{
    actions,
    app::App,
    config::{Config, CustomCommand, State},
    git::Repository,
    model::Panel,
};
use std::process::Command;

#[test]
fn footer_actions_match_the_full_catalog_prefix_in_every_pane_and_with_custom_bindings() {
    let directory = tempfile::tempdir().unwrap();
    assert!(
        Command::new("git")
            .args(["init", "-qb", "main"])
            .current_dir(directory.path())
            .status()
            .unwrap()
            .success()
    );
    let config = Config::load(Some(directory.path().join("config")), &[]).unwrap();
    let mut app = App::new(
        Repository::discover(directory.path()).unwrap(),
        config,
        State::default(),
    );
    app.workers.refresh.cancel();
    app.config.keybinding =
        serde_yaml::from_str("universal:\n  compareRevisions: '<disabled>'\n  pull: '<ctrl+g>'\n")
            .unwrap();
    app.config.custom_commands.push(CustomCommand {
        key: "x".into(),
        context: "global".into(),
        description: "Custom action".into(),
        command: "printf custom".into(),
        ..CustomCommand::default()
    });
    for panel in [
        Panel::Status,
        Panel::Files,
        Panel::Branches,
        Panel::RemoteBranches,
        Panel::Commits,
        Panel::Stash,
        Panel::CommitFiles,
        Panel::HeadFiles,
        Panel::Details,
        Panel::Output,
        Panel::Diff,
    ] {
        app.focus = panel;
        let full = actions::for_app(&app);
        let footer = actions::for_footer(&app);
        assert_eq!(footer.len(), full.len().min(5));
        for (actual, expected) in footer.iter().zip(full.iter()) {
            assert_eq!(
                (
                    &actual.key,
                    &actual.label,
                    &actual.description,
                    actual.event
                ),
                (
                    &expected.key,
                    &expected.label,
                    &expected.description,
                    expected.event
                )
            );
        }
    }
}
