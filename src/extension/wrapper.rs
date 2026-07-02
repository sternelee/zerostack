//! Extension tool wrapper — adapts a Wasm extension tool to rig's `ToolDyn` trait.

use std::pin::Pin;
use std::sync::Arc;

use rig::completion::ToolDefinition;
use rig::tool::{ToolDyn, ToolError};
use std::sync::Mutex;

use crate::extension::RegisteredTool;
use crate::extension::manager::ExtensionManager;

/// Wraps a extension-registered tool so it can be used as a `rig::tool::ToolDyn`
/// in the agent builder.
pub(crate) struct ExtensionToolWrapper {
    definition: RegisteredTool,
    manager: Arc<Mutex<ExtensionManager>>,
}

impl ExtensionToolWrapper {
    pub fn new(definition: RegisteredTool, manager: Arc<Mutex<ExtensionManager>>) -> Self {
        Self {
            definition,
            manager,
        }
    }
}

impl ToolDyn for ExtensionToolWrapper {
    fn name(&self) -> String {
        self.definition.name.clone()
    }

    fn definition<'a>(
        &'a self,
        _prompt: String,
    ) -> Pin<Box<dyn std::future::Future<Output = ToolDefinition> + Send + 'a>> {
        let name = self.definition.name.clone();
        let description = self.definition.description.clone();
        let params = serde_json::from_str(&self.definition.parameters_schema)
            .unwrap_or(serde_json::json!({"type": "object", "properties": {}}));

        Box::pin(async move {
            ToolDefinition {
                name,
                description,
                parameters: params,
            }
        })
    }

    fn call<'a>(
        &'a self,
        args: String,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<String, ToolError>> + Send + 'a>> {
        let tool_name = self.definition.name.clone();
        let manager = self.manager.clone();

        Box::pin(async move {
            // Use spawn_blocking since execute_tool is sync but we're in an async context.
            let result = tokio::task::spawn_blocking(move || {
                let mut mgr = manager.lock().unwrap();
                mgr.execute_tool(&tool_name, &args)
            })
            .await
            .map_err(|e| ToolError::ToolCallError(Box::new(e)))?;

            match result {
                Ok((content, details, is_error)) => {
                    let output = if details.is_empty() || details == "{}" {
                        content
                    } else {
                        format!("{content}\n\n<details>\n{details}\n</details>")
                    };
                    if is_error {
                        Err(ToolError::ToolCallError(anyhow::anyhow!(output).into()))
                    } else {
                        Ok(output)
                    }
                }
                Err(e) => Err(ToolError::ToolCallError(anyhow::anyhow!(e).into())),
            }
        })
    }
}
