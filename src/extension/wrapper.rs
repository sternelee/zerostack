//! Extension tool wrapper — adapts a Wasm extension tool to rig's `ToolDyn` trait.
//!
//! v0.5.0 changes:
//! - Validates `parameters_schema` JSON Schema against incoming args; rejects
//!   mismatches with a precise schema error before the Wasm call.
//! - Forwards `terminate` and `addedToolNames` as `<details>` markers; the
//!   agent runner reads them out of the response JSON.
//! - Carries `execution_mode` and `loading_mode` on the wrapper for future
//!   rig-pipeline integration.

use std::pin::Pin;
use std::sync::{Arc, Mutex};

use rig::tool::{ToolDyn, ToolError};

use crate::extension::RegisteredTool;
use crate::extension::manager::ExtensionManager;

/// Wrapper that bridges a Wasm extension's tool definition to rig's `ToolDyn`.
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

    /// Validate `args` against the tool's JSON Schema. Returns Ok(args) on
    /// success; Err on schema parse / validation failure.
    fn validate_args(&self, args: &serde_json::Value) -> Result<(), String> {
        let schema: serde_json::Value = serde_json::from_str(&self.definition.parameters_schema)
            .unwrap_or_else(|_| serde_json::json!({"type":"object","properties":{}}));
        validate_against_schema(args, &schema)
    }

    /// Test-only entry into schema validation (mirrors `validate_args`).
    #[cfg(test)]
    pub(crate) fn validate_args_for_test(&self, args: &serde_json::Value) -> Result<(), String> {
        self.validate_args(args)
    }
}

impl ToolDyn for ExtensionToolWrapper {
    fn name(&self) -> String {
        self.definition.name.clone()
    }

    fn description(&self) -> String {
        self.definition.description.clone()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::from_str(&self.definition.parameters_schema)
            .unwrap_or_else(|_| serde_json::json!({"type":"object","properties":{}}))
    }

    fn call<'a>(
        &'a self,
        args: String,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<String, ToolError>> + Send + 'a>> {
        let tool_name = self.definition.name.clone();
        let manager = self.manager.clone();
        // Best-effort server-side validation against the registered schema.
        let args_value: serde_json::Value =
            serde_json::from_str(&args).unwrap_or(serde_json::json!({"__raw__": args}));
        let validation = self.validate_args(&args_value);

        Box::pin(async move {
            // If the schema rejected the args, surface that error to the
            // model rather than wasting a Wasm call.
            if let Err(e) = validation {
                return Err(ToolError::ToolCallError(
                    anyhow::anyhow!("argument validation failed: {e}").into(),
                ));
            }

            let result = tokio::task::spawn_blocking(move || {
                let mut mgr = manager.lock().unwrap();
                mgr.execute_tool(&tool_name, &args)
            })
            .await
            .map_err(|e| ToolError::ToolCallError(Box::new(e)))?;

            match result {
                Ok((content, details, is_error, terminate, added)) => {
                    let mut details_obj: serde_json::Value = serde_json::from_str(&details)
                        .unwrap_or(serde_json::Value::String(details.clone()));
                    if !details_obj.is_object() {
                        details_obj = serde_json::json!({ "raw": details_obj });
                    }
                    let obj = details_obj.as_object_mut().expect("object coerced above");
                    if terminate {
                        obj.insert("__terminate__".into(), serde_json::Value::Bool(true));
                    }
                    if !added.is_empty() {
                        obj.insert(
                            "__added_tool_names__".into(),
                            serde_json::Value::Array(
                                added.into_iter().map(serde_json::Value::String).collect(),
                            ),
                        );
                    }
                    let details_json = serde_json::to_string(&details_obj).unwrap_or_default();

                    let output = if details_json == "{}" {
                        content
                    } else {
                        format!("{content}\n\n<details>\n{details_json}\n</details>")
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

// ── Minimal JSON Schema validator (subset of draft 2020-12) ──────────
//
// Implemented in-tree rather than pulling a full crate so we keep the binary
// slim. Supports: type, properties, required, enum, items.
// Does NOT support: $ref, allOf, anyOf, oneOf, not, conditional. Extensions
// needing those will fall back to schema-less validation.

fn validate_against_schema(
    value: &serde_json::Value,
    schema: &serde_json::Value,
) -> Result<(), String> {
    if let Some(t) = schema.get("type").and_then(|v| v.as_str()) {
        let ok = match t {
            "object" => value.is_object(),
            "array" => value.is_array(),
            "string" => value.is_string(),
            "number" => value.is_number(),
            "integer" => value.is_i64() || value.is_u64(),
            "boolean" => value.is_boolean(),
            "null" => value.is_null(),
            _ => true,
        };
        if !ok {
            return Err(format!("expected type '{t}', got {}", json_type(value)));
        }
    }
    if let Some(props) = schema.get("properties").and_then(|v| v.as_object()) {
        if let Some(obj) = value.as_object() {
            for (k, sub_schema) in props {
                if let Some(v) = obj.get(k) {
                    validate_against_schema(v, sub_schema)?;
                }
            }
        }
    }
    if let Some(req) = schema.get("required").and_then(|v| v.as_array()) {
        if let Some(obj) = value.as_object() {
            for k in req {
                if let Some(name) = k.as_str()
                    && !obj.contains_key(name)
                {
                    return Err(format!("missing required property '{name}'"));
                }
            }
        }
    }
    if let Some(enum_vals) = schema.get("enum").and_then(|v| v.as_array())
        && !enum_vals.iter().any(|v| v == value)
    {
        return Err("value not in enum".into());
    }
    if let Some(items) = schema.get("items")
        && let Some(arr) = value.as_array()
    {
        for (i, item) in arr.iter().enumerate() {
            validate_against_schema(item, items).map_err(|e| format!("items[{i}]: {e}"))?;
        }
    }
    Ok(())
}

fn json_type(v: &serde_json::Value) -> &'static str {
    match v {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}
