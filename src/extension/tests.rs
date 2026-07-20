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
        assert!(meta
            .command_names
            .iter()
            .any(|n| n.contains("session_name__name")));
        assert!(meta
            .tool_names
            .iter()
            .any(|n| n.contains("set_session_name")));
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
}
