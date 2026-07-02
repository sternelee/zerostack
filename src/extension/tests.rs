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

    fn btw_extension_path() -> std::path::PathBuf {
        let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        manifest_dir.join("tests/extensions/btw/target/wasm32-unknown-unknown/release/btw.wasm")
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

    #[test]
    fn test_load_btw_extension() {
        let path = btw_extension_path();
        if !path.exists() {
            return;
        }
        let mut manager = ExtensionManager::new().unwrap();
        let meta = manager.load_standalone(&path).unwrap();
        assert!(meta.tool_names.contains(&"btw_ask".to_string()));
        assert!(meta.command_names.contains(&"btw".to_string()));
    }

    #[test]
    fn test_btw_command_dispatch() {
        let path = btw_extension_path();
        if !path.exists() {
            return;
        }
        let mut manager = ExtensionManager::new().unwrap();
        manager.load_standalone(&path).unwrap();
        let result = manager.dispatch_command("btw", "help").unwrap();
        assert!(result.is_some());
        assert!(result.unwrap().contains("BTW"));
    }

    #[test]
    fn test_echo_and_btw_together() {
        let echo_path = test_extension_path();
        let btw_path = btw_extension_path();
        if !echo_path.exists() || !btw_path.exists() {
            return;
        }

        let mut manager = ExtensionManager::new().unwrap();
        manager.load_standalone(&echo_path).unwrap();
        manager.load_standalone(&btw_path).unwrap();

        // Both extensions loaded, both tools available.
        let tools = manager.all_tools();
        assert!(tools.iter().any(|t| t.name == "echo"));
        assert!(tools.iter().any(|t| t.name == "btw_ask"));

        // Commands from btw available.
        let cmd_result = manager.dispatch_command("btw", "extension test").unwrap();
        assert!(cmd_result.is_some());
    }
}
