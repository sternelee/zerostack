//! Integration tests for the extension system.

#[cfg(feature = "extensions")]
#[cfg(test)]
mod tests {
    use std::sync::OnceLock;

    use crate::extension::manager::ExtensionManager;

    static TEST_EXTENSION_ARTIFACT: OnceLock<std::path::PathBuf> = OnceLock::new();

    fn test_extension_path() -> &'static std::path::PathBuf {
        TEST_EXTENSION_ARTIFACT.get_or_init(|| {
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
        })
    }

    #[test]
    fn test_load_echo_extension() {
        let path = test_extension_path();
        let mut manager = ExtensionManager::new().unwrap();
        let meta = manager.load_standalone(path).unwrap();
        assert!(meta.tool_names.contains(&"echo".to_string()));
    }

    #[test]
    fn test_execute_echo_tool() {
        let path = test_extension_path();
        let mut manager = ExtensionManager::new().unwrap();
        manager.load_standalone(path).unwrap();
        let (content, _, is_error) = manager
            .execute_tool("echo", r#"{"message":"hello"}"#)
            .unwrap();
        assert!(!is_error);
        assert!(content.contains("hello"));
    }
}
