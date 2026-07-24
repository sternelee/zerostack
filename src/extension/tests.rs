//! Integration tests for the extension system.

#[cfg(feature = "extensions")]
#[cfg(test)]
mod tests {
    use std::sync::LazyLock;

    use crate::extension::manager::ExtensionManager;

    static TEST_EXTENSION_ARTIFACT: LazyLock<std::path::PathBuf> = LazyLock::new(|| {
        let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let artifact = manifest_dir
            .join("target")
            .join("wasm32-wasip2")
            .join("debug")
            .join("test_echo.wasm");

        if !artifact.exists() {
            // Build the test extension on demand. This runs outside the
            // parent cargo invocation, so it can acquire the workspace lock.
            let status = std::process::Command::new("cargo")
                .args(["build", "-p", "test-echo", "--target", "wasm32-wasip2"])
                .status()
                .expect("failed to invoke cargo to build test-echo extension");
            assert!(
                status.success(),
                "failed to build test-echo extension for integration tests"
            );
        }

        assert!(
            artifact.exists(),
            "test extension artifact not found at {artifact:?}"
        );
        artifact
    });

    #[test]
    fn test_load_echo_extension() {
        let path = &*TEST_EXTENSION_ARTIFACT;
        let mut manager = ExtensionManager::new().unwrap();
        let meta = manager.load_standalone(path).unwrap();
        // Tools are now namespaced as `test_echo__echo`.
        assert!(meta.tool_names.iter().any(|n| n.ends_with("echo")));
    }

    #[test]
    fn test_execute_echo_tool() {
        let path = &*TEST_EXTENSION_ARTIFACT;
        let mut manager = ExtensionManager::new().unwrap();
        manager.load_standalone(path).unwrap();
        // Use namespaced tool name.
        let (content, _, is_error) = manager
            .execute_tool("test_echo__echo", r#"{"message":"hello"}"#)
            .unwrap();
        assert!(!is_error);
        assert!(content.contains("hello"));
    }

    #[test]
    fn test_bare_tool_name_resolution() {
        let path = &*TEST_EXTENSION_ARTIFACT;
        let mut manager = ExtensionManager::new().unwrap();
        manager.load_standalone(path).unwrap();
        // Bare name should resolve when unambiguous.
        let (content, _, is_error) = manager
            .execute_tool("echo", r#"{"message":"bare-name-test"}"#)
            .unwrap();
        assert!(!is_error);
        assert!(content.contains("bare-name-test"));
    }

    #[test]
    fn test_context_in_tool() {
        let path = &*TEST_EXTENSION_ARTIFACT;
        let mut manager = ExtensionManager::new().unwrap();
        manager.update_context("/test/cwd", "test-session-id", "test-model", true);
        manager.load_standalone(path).unwrap();
        let (content, _, is_error) = manager
            .execute_tool("echo", r#"{"message":"ctx-test"}"#)
            .unwrap();
        assert!(!is_error);
        assert!(content.contains("cwd: /test/cwd"));
        assert!(content.contains("session: test-session-id"));
        assert!(content.contains("model: test-model"));
        assert!(content.contains("trusted: true"));
    }

    static SESSION_NAME_ARTIFACT: LazyLock<std::path::PathBuf> = LazyLock::new(|| {
        let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let artifact = manifest_dir
            .join("target")
            .join("wasm32-wasip2")
            .join("debug")
            .join("session_name.wasm");

        if !artifact.exists() {
            let status = std::process::Command::new("cargo")
                .args(["build", "-p", "session-name", "--target", "wasm32-wasip2"])
                .status()
                .expect("failed to invoke cargo to build session-name extension");
            assert!(
                status.success(),
                "failed to build session-name extension for integration tests"
            );
        }

        assert!(
            artifact.exists(),
            "session-name artifact not found at {artifact:?}"
        );
        artifact
    });

    #[test]
    fn test_load_session_name_extension() {
        let path = &*SESSION_NAME_ARTIFACT;
        let mut manager = ExtensionManager::new().unwrap();
        let meta = manager.load_standalone(path).unwrap();
        assert!(
            meta.command_names
                .iter()
                .any(|n| n.contains("session_name__name"))
        );
        assert!(
            meta.tool_names
                .iter()
                .any(|n| n.contains("set_session_name"))
        );
    }

    #[test]
    fn test_set_and_get_session_name() {
        let path = &*SESSION_NAME_ARTIFACT;
        let mut manager = ExtensionManager::new().unwrap();
        manager.load_standalone(path).unwrap();

        // Start with empty session name.
        assert_eq!(manager.get_session_name(), "");

        // Set session name via tool.
        let (content, _, is_error) = manager
            .execute_tool(
                "session_name__set_session_name",
                r#"{"name":"My Test Session"}"#,
            )
            .unwrap();
        assert!(!is_error);
        assert!(content.contains("My Test Session"));

        // Verify session name was set.
        assert_eq!(manager.get_session_name(), "My Test Session");
    }

    #[test]
    fn test_name_command_sets_directly() {
        let path = &*SESSION_NAME_ARTIFACT;
        let mut manager = ExtensionManager::new().unwrap();
        manager.load_standalone(path).unwrap();

        // Use /name command with direct argument.
        let output = manager
            .dispatch_command("session_name__name", "Direct Session Name")
            .unwrap();
        assert!(output.is_some());
        assert!(output.unwrap().contains("Direct Session Name"));
        assert_eq!(manager.get_session_name(), "Direct Session Name");
    }

    #[test]
    fn test_name_command_shows_existing() {
        let path = &*SESSION_NAME_ARTIFACT;
        let mut manager = ExtensionManager::new().unwrap();
        manager.load_standalone(path).unwrap();

        // Set a name first.
        manager.set_session_name("Existing Name");

        // /name without args should show the existing name.
        let output = manager.dispatch_command("session_name__name", "").unwrap();
        assert!(output.is_some());
        assert!(output.unwrap().contains("Existing Name"));
    }

    #[test]
    fn test_name_command_triggers_prompt_when_empty() {
        let path = &*SESSION_NAME_ARTIFACT;
        let mut manager = ExtensionManager::new().unwrap();
        manager.load_standalone(path).unwrap();

        // /name without args and no existing name should trigger a prompt.
        let output = manager.dispatch_command("session_name__name", "").unwrap();
        assert!(output.is_some());
        assert!(output.unwrap().contains("generate a session name"));

        let prompts = manager.take_queued_prompts();
        assert!(!prompts.is_empty());
        assert!(prompts[0].contains("short, concise session title"));
    }

    static ADD_DIR_ARTIFACT: LazyLock<std::path::PathBuf> = LazyLock::new(|| {
        let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let artifact = manifest_dir
            .join("target")
            .join("wasm32-wasip2")
            .join("debug")
            .join("add_dir.wasm");

        if !artifact.exists() {
            let status = std::process::Command::new("cargo")
                .args(["build", "-p", "add-dir", "--target", "wasm32-wasip2"])
                .status()
                .expect("failed to invoke cargo to build add-dir extension");
            assert!(
                status.success(),
                "failed to build add-dir extension for integration tests"
            );
        }

        assert!(
            artifact.exists(),
            "add-dir artifact not found at {artifact:?}"
        );
        artifact
    });

    /// Use a unique tempdir under target/ so the integration test is hermetic
    /// and doesn't depend on the host's real cwd.
    fn fresh_tmpdir(label: &str) -> std::path::PathBuf {
        let base = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-tmpdirs");
        std::fs::create_dir_all(&base).unwrap();
        let dir = base.join(format!(
            "{label}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn test_load_add_dir_extension() {
        let path = &*ADD_DIR_ARTIFACT;
        let mut manager = ExtensionManager::new().unwrap();
        let meta = manager.load_standalone(path).unwrap();
        assert!(meta.command_names.iter().any(|n| n.contains("add-dir")));
        assert!(meta.command_names.iter().any(|n| n.contains("remove-dir")));
        assert!(
            meta.command_names
                .iter()
                .any(|n| n.ends_with("dirs") || n.contains("__dirs"))
        );
        assert!(meta.tool_names.iter().any(|n| n.ends_with("add_directory")));
        assert!(
            meta.tool_names
                .iter()
                .any(|n| n.ends_with("remove_directory"))
        );
        assert!(
            meta.tool_names
                .iter()
                .any(|n| n.ends_with("list_directories"))
        );
    }

    #[test]
    fn test_add_dir_tool_round_trip() {
        let path = &*ADD_DIR_ARTIFACT;
        let mut manager = ExtensionManager::new().unwrap();

        // Pin the manager's cwd to a hermetic tempdir so the add-dir path
        // resolution does not depend on the host filesystem.
        let tmp = fresh_tmpdir("add-dir-roundtrip");
        manager.update_context(&tmp.to_string_lossy(), "test-session", "test-model", true);

        manager.load_standalone(path).unwrap();

        // Initially no external dirs.
        assert!(manager.external_dirs().is_empty());

        let (content, _, is_error) = manager
            .execute_tool(
                "add_dir__add_directory",
                &format!(r#"{{"path":"{0}"}}"#, tmp.to_string_lossy()),
            )
            .unwrap();
        assert!(!is_error, "{content}");
        assert!(content.contains("added"));

        let dirs = manager.external_dirs();
        assert_eq!(dirs.len(), 1);
        assert_eq!(dirs[0], tmp.canonicalize().unwrap().to_string_lossy());

        // Listing via the tool returns the same dir.
        let (list_content, _, is_error) = manager
            .execute_tool("add_dir__list_directories", "{}")
            .unwrap();
        assert!(!is_error, "{list_content}");
        assert!(list_content.contains("1 external directory"));

        // Removing via the tool removes the entry.
        let (rm_content, _, is_error) = manager
            .execute_tool(
                "add_dir__remove_directory",
                &format!(r#"{{"path":"{0}"}}"#, tmp.to_string_lossy()),
            )
            .unwrap();
        assert!(!is_error, "{rm_content}");
        assert!(manager.external_dirs().is_empty());
    }

    #[test]
    fn test_add_dir_command_lists_current_dirs() {
        let path = &*ADD_DIR_ARTIFACT;
        let mut manager = ExtensionManager::new().unwrap();
        let tmp = fresh_tmpdir("add-dir-cmd-list");
        manager.update_context(&tmp.to_string_lossy(), "test-session", "test-model", true);
        manager.load_standalone(path).unwrap();

        // Pre-add via the tool so we have a known state to render.
        manager
            .execute_tool(
                "add_dir__add_directory",
                &format!(r#"{{"path":"{0}"}}"#, tmp.to_string_lossy()),
            )
            .unwrap();

        let output = manager
            .dispatch_command("add_dir__dirs", "")
            .unwrap()
            .expect("dirs command handled");
        assert!(output.contains("external directory"));
        assert!(output.contains(&tmp.canonicalize().unwrap().to_string_lossy().to_string()));

        // /dirs with empty list returns the empty hint.
        manager
            .execute_tool(
                "add_dir__remove_directory",
                &format!(r#"{{"path":"{0}"}}"#, tmp.to_string_lossy()),
            )
            .unwrap();
        let output = manager
            .dispatch_command("add_dir__dirs", "")
            .unwrap()
            .expect("dirs command handled");
        assert!(output.contains("no external directories"));
    }

    #[test]
    fn test_remove_dir_command_shows_paths() {
        let path = &*ADD_DIR_ARTIFACT;
        let mut manager = ExtensionManager::new().unwrap();
        let tmp = fresh_tmpdir("add-dir-rm-show");
        manager.update_context(&tmp.to_string_lossy(), "test-session", "test-model", true);
        manager.load_standalone(path).unwrap();

        // No args + list is empty → "nothing to remove".
        let output = manager
            .dispatch_command("add_dir__remove-dir", "")
            .unwrap()
            .expect("handled");
        assert!(output.contains("nothing to remove"));

        // Add one then run /remove-dir with no args → list.
        manager
            .execute_tool(
                "add_dir__add_directory",
                &format!(r#"{{"path":"{0}"}}"#, tmp.to_string_lossy()),
            )
            .unwrap();
        let output = manager
            .dispatch_command("add_dir__remove-dir", "")
            .unwrap()
            .expect("handled");
        assert!(output.contains("current directories:"));
    }

    #[test]
    fn test_add_dir_command_missing_arg_shows_suggestions() {
        let path = &*ADD_DIR_ARTIFACT;
        let mut manager = ExtensionManager::new().unwrap();
        let tmp = fresh_tmpdir("add-dir-suggest");
        manager.update_context(&tmp.to_string_lossy(), "test-session", "test-model", true);
        manager.load_standalone(path).unwrap();

        let output = manager
            .dispatch_command("add_dir__add-dir", "")
            .unwrap()
            .expect("handled");
        // Either we get a 'usage' line (no suggestions) or a 'suggestions' list.
        assert!(
            output.starts_with("usage") || output.contains("suggestions"),
            "unexpected output: {output}",
        );
    }

    #[test]
    fn test_no_such_directory_is_error() {
        let path = &*ADD_DIR_ARTIFACT;
        let mut manager = ExtensionManager::new().unwrap();
        let tmp = fresh_tmpdir("add-dir-noexist");
        manager.update_context(&tmp.to_string_lossy(), "test-session", "test-model", true);
        manager.load_standalone(path).unwrap();

        let bogus = tmp.join("does-not-exist-xyz");
        let err = manager
            .execute_tool(
                "add_dir__add_directory",
                &format!(r#"{{"path":"{0}"}}"#, bogus.to_string_lossy()),
            )
            .unwrap_err();
        assert!(err.contains("cannot add") || err.contains("does-not-exist"));
    }

    // ── host-calls / pi-simplify ──────────────────────────────

    static PI_SIMPLIFY_ARTIFACT: LazyLock<std::path::PathBuf> = LazyLock::new(|| {
        let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let artifact = manifest_dir
            .join("target")
            .join("wasm32-wasip2")
            .join("debug")
            .join("pi_simplify.wasm");

        if !artifact.exists() {
            let status = std::process::Command::new("cargo")
                .args(["build", "-p", "pi-simplify", "--target", "wasm32-wasip2"])
                .status()
                .expect("failed to invoke cargo to build pi-simplify extension");
            assert!(
                status.success(),
                "failed to build pi-simplify extension for integration tests"
            );
        }

        assert!(
            artifact.exists(),
            "pi-simplify artifact not found at {artifact:?}"
        );
        artifact
    });

    #[test]
    fn test_load_pi_simplify_extension() {
        let path = &*PI_SIMPLIFY_ARTIFACT;
        let mut manager = ExtensionManager::new().unwrap();
        let meta = manager.load_standalone(path).unwrap();
        assert!(
            meta.command_names.iter().any(|n| n.ends_with("__simplify")),
            "pi-simplify did not register /simplify command: {:?}",
            meta.command_names
        );
    }

    /// `/simplify` runs `git diff --name-status` via `host-calls::exec`.
    /// This was a runtime regression: `std::process::Command` on the
    /// wasm32-wasip2 guest previously returned ENOSYS for `sh -c`,
    /// because `wasi:cli/process` was never installed in the linker.
    /// The fix routes through the host's stdlib via the new
    /// `host-calls` interface. This test verifies the end-to-end path
    /// against a hermetic temp git repo.
    #[test]
    fn test_pi_simplify_runs_git_via_host_calls() {
        // Skip if git is not available on the host — the integration is
        // about pi-simplify *invoking* git, not about git itself.
        let git_ok = std::process::Command::new("git")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !git_ok {
            eprintln!("skipping: git not available on host");
            return;
        }

        // Build the wasm artifact in case it's not already there.
        let _ = &*PI_SIMPLIFY_ARTIFACT;

        // Hermetic git repo with a single committed file and one
        // unstaged change, so `git diff` returns a known entry.
        let dir = fresh_tmpdir("pi-simplify-git");
        let run_git = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(&dir)
                .output()
                .unwrap_or_else(|e| panic!("git {args:?} failed: {e}"))
        };
        run_git(&["init", "-q"]);
        run_git(&["config", "user.email", "test@example.com"]);
        run_git(&["config", "user.name", "test"]);
        std::fs::write(dir.join("stale.txt"), "old contents\n").unwrap();
        run_git(&["add", "stale.txt"]);
        run_git(&["commit", "-q", "-m", "init"]);
        std::fs::write(dir.join("stale.txt"), "new contents\n").unwrap();

        let path = &*PI_SIMPLIFY_ARTIFACT;
        let mut manager = ExtensionManager::new().unwrap();
        manager.update_context(&dir.to_string_lossy(), "test-session", "test-model", true);
        manager.load_standalone(path).unwrap();

        // The /simplify command should now succeed (it used to fail
        // with "operation not supported on this platform") and report
        // exactly the unstaged changed file.
        let output = manager
            .dispatch_command("pi_simplify__simplify", "")
            .unwrap()
            .expect("simplify command handled");

        assert!(
            !output.contains("operation not supported"),
            "/simplify fell back to WASI subprocess: {output}"
        );
        assert!(
            !output.contains("failed to run command"),
            "/simplify failed: {output}"
        );
        assert!(
            output.contains("stale.txt"),
            "/simplify output missing changed file: {output}"
        );
    }
}
