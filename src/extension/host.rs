//! Extension host — wasmtime-based Wasm runtime for extensions.
//!
//! Supports tool registration, command registration, and tool execution.

use std::collections::HashMap;
use std::path::Path;
use std::process::Command as StdCommand;

use wasmtime::*;

use crate::extension::loader::Capabilities;
use crate::extension::{ExtensionId, ExtensionMeta, RegisteredCommand, RegisteredTool};

// ── ExtensionHost ──────────────────────────────────────────────

pub(crate) struct ExtensionHost {
    engine: Engine,
    instances: HashMap<ExtensionId, LoadedExtension>,
}

struct LoadedExtension {
    store: Store<ExtGuestState>,
    meta: ExtensionMeta,
    instance: Instance,
}

pub(crate) struct ExtGuestState {
    pub extension_id: ExtensionId,
    pub tools: Vec<RegisteredTool>,
    pub subscriptions: Vec<String>,
    /// Command name → description (for slash command autocomplete).
    pub commands: HashMap<String, String>,
}

impl ExtGuestState {
    fn new(extension_id: &str) -> Self {
        Self {
            extension_id: extension_id.to_string(),
            tools: Vec::new(),
            subscriptions: Vec::new(),
            commands: HashMap::new(),
        }
    }
}

impl ExtensionHost {
    pub fn new() -> Result<Self, String> {
        let mut config = Config::default();
        config.consume_fuel(true);
        let engine = Engine::new(&config).map_err(|e| e.to_string())?;
        Ok(Self {
            engine,
            instances: HashMap::new(),
        })
    }

    /// Load a extension from a .wasm file.
    pub fn load_extension(
        &mut self,
        extension_id: &str,
        wasm_path: &Path,
        _capabilities: &Capabilities,
    ) -> Result<ExtensionMeta, String> {
        let wasm_bytes =
            std::fs::read(wasm_path).map_err(|e| format!("failed to read {wasm_path:?}: {e}"))?;

        let module = Module::from_binary(&self.engine, &wasm_bytes)
            .map_err(|e| format!("failed to compile: {e}"))?;

        let mut store = Store::new(&self.engine, ExtGuestState::new(extension_id));

        // ── Host imports ──────────────────────────────────────

        let host_register_tool = Func::wrap(&mut store, {
            move |mut caller: Caller<'_, ExtGuestState>,
                  def_ptr: i32,
                  def_len: i32|
                  -> wasmtime::Result<i32> {
                let mem = caller
                    .get_export("memory")
                    .and_then(|e| e.into_memory())
                    .ok_or_else(|| wasmtime::Error::msg("no memory export"))?;

                let len = def_len as usize;
                let mut buf = vec![0u8; len.min(65536)];
                mem.read(&caller, def_ptr as usize, &mut buf)?;
                let json_str = std::str::from_utf8(&buf)
                    .map_err(|e| wasmtime::Error::msg(format!("utf8: {e}")))?;

                #[derive(serde::Deserialize)]
                struct RawDef {
                    name: String,
                    label: String,
                    description: String,
                    parameters_schema: String,
                    #[serde(default)]
                    prompt_snippet: Option<String>,
                    #[serde(default)]
                    prompt_guidelines: Vec<String>,
                }
                let def: RawDef = serde_json::from_str(json_str)
                    .map_err(|e| wasmtime::Error::msg(format!("invalid tool def: {e}")))?;

                let state = caller.data_mut();
                state.tools.push(RegisteredTool {
                    name: def.name.clone(),
                    label: def.label,
                    description: def.description,
                    parameters_schema: def.parameters_schema,
                    prompt_snippet: def.prompt_snippet,
                    prompt_guidelines: def.prompt_guidelines,
                    extension_id: state.extension_id.clone(),
                });
                Ok(0)
            }
        });

        let host_register_command = Func::wrap(&mut store, {
            move |mut caller: Caller<'_, ExtGuestState>,
                  def_ptr: i32,
                  def_len: i32|
                  -> wasmtime::Result<i32> {
                let mem = caller
                    .get_export("memory")
                    .and_then(|e| e.into_memory())
                    .ok_or_else(|| wasmtime::Error::msg("no memory export"))?;

                let len = def_len as usize;
                let mut buf = vec![0u8; len.min(4096)];
                mem.read(&caller, def_ptr as usize, &mut buf)?;
                let json_str = std::str::from_utf8(&buf)
                    .map_err(|e| wasmtime::Error::msg(format!("utf8: {e}")))?;

                #[derive(serde::Deserialize)]
                struct CmdDef {
                    name: String,
                    description: String,
                }
                let def: CmdDef = serde_json::from_str(json_str)
                    .map_err(|e| wasmtime::Error::msg(format!("invalid cmd def: {e}")))?;

                let state = caller.data_mut();
                state.commands.insert(def.name.clone(), def.description);
                Ok(0)
            }
        });

        let host_exec = Func::wrap(&mut store, {
            move |mut caller: Caller<'_, ExtGuestState>,
                  cmd_ptr: i32,
                  cmd_len: i32,
                  result_ptr: i32,
                  result_max: i32|
                  -> wasmtime::Result<i32> {
                let mem = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                    Some(m) => m,
                    None => return Ok(-1),
                };

                let cmd_len = cmd_len as usize;
                let mut cmd_buf = vec![0u8; cmd_len.min(4096)];
                let _ = mem.read(&caller, cmd_ptr as usize, &mut cmd_buf);

                // Parse null-delimited command: "sh\0-c\0grep ...\0"
                let cmd_str = String::from_utf8_lossy(&cmd_buf);
                let parts: Vec<&str> = cmd_str.split('\0').filter(|s| !s.is_empty()).collect();

                if parts.len() < 2 {
                    return Ok(-1);
                }

                let shell = parts[0];
                let arg = parts[1];
                let rest = if parts.len() > 2 { parts[2] } else { "" };
                let full_cmd = format!("{arg} {rest}");

                let output = std::process::Command::new(shell)
                    .arg("-c")
                    .arg(&full_cmd)
                    .output();

                let (exit_code, stdout) = match output {
                    Ok(o) => (
                        o.status.code().unwrap_or(-1),
                        String::from_utf8_lossy(&o.stdout).to_string(),
                    ),
                    Err(_) => return Ok(-99),
                };

                let result_bytes = stdout.as_bytes();
                let write_len = result_bytes.len().min(result_max as usize);
                let _ = mem.write(&mut caller, result_ptr as usize, &result_bytes[..write_len]);
                Ok(exit_code)
            }
        });

        let instance = Instance::new(
            &mut store,
            &module,
            &[
                host_register_tool.into(),
                host_register_command.into(),
                host_exec.into(),
            ],
        )
        .map_err(|e| format!("instantiation failed: {e}"))?;

        // Call init()
        let init_fn = instance
            .get_typed_func::<(), i32>(&mut store, "init")
            .map_err(|e| format!("no init export: {e}"))?;
        let code = init_fn
            .call(&mut store, ())
            .map_err(|e| format!("init trap: {e}"))?;
        if code != 0 {
            return Err(format!("extension init returned error code {code}"));
        }

        let state = store.data();
        let tools = state.tools.clone();
        let command_names: Vec<String> = state.commands.keys().cloned().collect();

        let meta = ExtensionMeta {
            id: extension_id.to_string(),
            name: extension_id.to_string(),
            version: String::new(),
            description: String::new(),
            tool_names: tools.iter().map(|t| t.name.clone()).collect(),
            command_names,
            subscriptions: Vec::new(),
        };

        self.instances.insert(
            extension_id.to_string(),
            LoadedExtension {
                store,
                instance,
                meta: meta.clone(),
            },
        );
        Ok(meta)
    }

    /// Execute a extension-registered tool.
    pub fn execute_tool(
        &mut self,
        extension_id: &str,
        tool_name: &str,
        params_json: &str,
    ) -> Result<ToolOutput, String> {
        let loaded = self
            .instances
            .get_mut(extension_id)
            .ok_or_else(|| format!("extension '{extension_id}' not loaded"))?;

        let name_bytes = tool_name.as_bytes();
        let params_bytes = params_json.as_bytes();
        let mem = loaded
            .instance
            .get_memory(&mut loaded.store, "memory")
            .ok_or_else(|| "extension has no memory export".to_string())?;

        let name_ptr = guest_alloc(&mut loaded.store, &loaded.instance, name_bytes.len())
            .map_err(|e| format!("alloc failed: {e}"))?;
        mem.write(&mut loaded.store, name_ptr, name_bytes)
            .map_err(|e| format!("write: {e}"))?;

        let params_ptr = guest_alloc(&mut loaded.store, &loaded.instance, params_bytes.len())
            .map_err(|e| format!("alloc failed: {e}"))?;
        mem.write(&mut loaded.store, params_ptr, params_bytes)
            .map_err(|e| format!("write: {e}"))?;

        let tool_fn = loaded
            .instance
            .get_typed_func::<(i32, i32, i32, i32), i64>(&mut loaded.store, "tool_execute")
            .map_err(|e| format!("no tool_execute export: {e}"))?;

        let packed = tool_fn
            .call(
                &mut loaded.store,
                (
                    name_ptr as i32,
                    name_bytes.len() as i32,
                    params_ptr as i32,
                    params_bytes.len() as i32,
                ),
            )
            .map_err(|e| format!("tool_execute trap: {e}"))?;

        read_string_result(&loaded.instance, &mut loaded.store, &mem, packed)
    }

    /// Dispatch a registered slash command.
    pub fn dispatch_command(
        &mut self,
        command_name: &str,
        args: &str,
    ) -> Result<Option<String>, String> {
        // Find which extension registered this command.
        let extension_id = {
            let mut found = None;
            for (id, inst) in &self.instances {
                if inst.store.data().commands.contains_key(command_name) {
                    found = Some(id.clone());
                    break;
                }
            }
            found
        };

        let Some(extension_id) = extension_id else {
            return Ok(None);
        };

        let loaded = self
            .instances
            .get_mut(&extension_id)
            .ok_or_else(|| format!("extension vanished"))?;

        // Check if extension exports on_command.
        let cmd_fn = match loaded
            .instance
            .get_typed_func::<(i32, i32, i32, i32), i64>(&mut loaded.store, "on_command")
        {
            Ok(f) => f,
            Err(_) => return Ok(None),
        };

        let name_bytes = command_name.as_bytes();
        let args_bytes = args.as_bytes();
        let mem = loaded
            .instance
            .get_memory(&mut loaded.store, "memory")
            .ok_or_else(|| "no memory".to_string())?;

        let name_ptr = guest_alloc(&mut loaded.store, &loaded.instance, name_bytes.len())?;
        mem.write(&mut loaded.store, name_ptr, name_bytes).ok();

        let args_ptr = guest_alloc(&mut loaded.store, &loaded.instance, args_bytes.len())?;
        mem.write(&mut loaded.store, args_ptr, args_bytes).ok();

        let packed = cmd_fn
            .call(
                &mut loaded.store,
                (
                    name_ptr as i32,
                    name_bytes.len() as i32,
                    args_ptr as i32,
                    args_bytes.len() as i32,
                ),
            )
            .map_err(|e| format!("on_command trap: {e}"))?;

        let result = read_string_result(&loaded.instance, &mut loaded.store, &mem, packed)?;
        Ok(Some(result.content))
    }

    pub fn all_commands(&self) -> Vec<RegisteredCommand> {
        let mut cmds = Vec::new();
        for (pid, inst) in &self.instances {
            let state = inst.store.data();
            for (name, desc) in &state.commands {
                cmds.push(RegisteredCommand {
                    name: name.clone(),
                    description: desc.clone(),
                    extension_id: pid.clone(),
                });
            }
        }
        cmds
    }

    pub fn unload_extension(&mut self, extension_id: &str) -> Option<ExtensionMeta> {
        self.instances.remove(extension_id).map(|i| i.meta)
    }

    pub fn all_tools(&self) -> Vec<RegisteredTool> {
        let mut tools = Vec::new();
        for inst in self.instances.values() {
            tools.extend(inst.store.data().tools.clone());
        }
        tools
    }
}

// ── Helpers ─────────────────────────────────────────────────

fn guest_alloc(
    store: &mut Store<ExtGuestState>,
    instance: &Instance,
    size: usize,
) -> Result<usize, String> {
    let alloc_fn = instance
        .get_typed_func::<i32, i32>(&mut *store, "alloc")
        .map_err(|e| format!("no alloc export: {e}"))?;
    let ptr = alloc_fn
        .call(store, size as i32)
        .map_err(|e| format!("alloc trap: {e}"))?;
    Ok(ptr as usize)
}

fn read_string_result(
    _instance: &Instance,
    store: &mut Store<ExtGuestState>,
    mem: &Memory,
    packed: i64,
) -> Result<ToolOutput, String> {
    let result_ptr = ((packed >> 32) & 0xFFFF_FFFF) as usize;
    let result_len = (packed & 0xFFFF_FFFF) as usize;

    if result_len == 0 || result_len > 65536 {
        return Ok(ToolOutput::empty());
    }

    let mut buf = vec![0u8; result_len];
    mem.read(store, result_ptr, &mut buf)
        .map_err(|e| format!("read: {e}"))?;
    let json = std::str::from_utf8(&buf).map_err(|e| format!("utf8: {e}"))?;

    #[derive(serde::Deserialize)]
    struct R {
        #[serde(default)]
        content: String,
        #[serde(default)]
        details: String,
        #[serde(default)]
        is_error: bool,
    }
    let r: R = serde_json::from_str(json).map_err(|e| format!("json: {e}"))?;
    Ok(ToolOutput {
        content: r.content,
        details: r.details,
        is_error: r.is_error,
    })
}

/// Execute a shell command and return stdout.
#[allow(dead_code)]
pub fn exec_shell(cmd: &str) -> Result<String, String> {
    let output = StdCommand::new("sh")
        .arg("-c")
        .arg(cmd)
        .output()
        .map_err(|e| format!("exec failed: {e}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if !output.status.success() {
        Ok(format!("{stdout}\n{stderr}"))
    } else {
        Ok(stdout)
    }
}

#[derive(Debug, Clone)]
pub struct ToolOutput {
    pub content: String,
    pub details: String,
    pub is_error: bool,
}

impl ToolOutput {
    fn empty() -> Self {
        Self {
            content: String::new(),
            details: "{}".into(),
            is_error: false,
        }
    }
}
