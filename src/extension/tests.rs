//! Integration tests for the extension system.

#[cfg(feature = "extensions")]
#[cfg(test)]
mod tests {
    use crate::extension::manager::ExtensionManager;

    fn test_extension_path() -> std::path::PathBuf {
        let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        manifest_dir
            .join("tests/extensions/test-echo/target/wasm32-unknown-unknown/release/test_echo.wasm")
    }

    #[test]
    fn test_load_echo_extension() {
        let path = test_extension_path();
        if !path.exists() {
            return;
        }
        let mut manager = ExtensionManager::new().unwrap();
        let meta = manager.load_standalone(&path).unwrap();
        assert!(meta.tool_names.contains(&"echo".to_string()));
    }

    #[test]
    fn test_execute_echo_tool() {
        let path = test_extension_path();
        if !path.exists() {
            return;
        }
        let mut manager = ExtensionManager::new().unwrap();
        manager.load_standalone(&path).unwrap();
        let (content, _, is_error) = manager
            .execute_tool("echo", r#"{"message":"hello"}"#)
            .unwrap();
        assert!(!is_error);
        assert!(content.contains("hello"));
    }
}
