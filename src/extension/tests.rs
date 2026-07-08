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
}
