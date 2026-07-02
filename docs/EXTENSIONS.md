# Plugin System Design — zerostack

> **设计原则：** API 设计借鉴 pi 的 `ExtensionAPI`（事件驱动 + 工具/命令注册），
> 开发与打包模型借鉴 Zed（Rust → Wasm 编译、`extension.toml` 清单、registry 分发）。

## 目录

1. [架构概述](#1-架构概述)
2. [WIT 接口定义](#2-wit-接口定义)
3. [插件清单 — extension.toml](#3-插件清单--extensiontoml)
4. [插件运行时架构](#4-插件运行时架构)
5. [事件生命周期](#5-事件生命周期)
6. [集成点设计](#6-集成点设计)
7. [打包与分发](#7-打包与分发)
8. [CLI 命令](#8-cli-命令)
9. [实现路线](#9-实现路线)
10. [附录：与现有 extras 模块的关系](#10-附录与现有-extras-模块的关系)

---

## 1. 架构概述

### 1.1 总览

```
┌──────────────────────────────────────────────────────────┐
│                      zerostack Host                       │
│                                                          │
│  ┌──────────────────────┐    ┌──────────────────────────┐│
│  │   Plugin Manager      │    │    PluginHost (wasmtime)  ││
│  │   - 发现 & 加载       │    │    - Engine 池            ││
│  │   - 生命周期管理      │◄──►│    - 实例化 & 隔离        ││
│  │   - 事件路由          │    │    - WIT bindings         ││
│  └──────┬───────────────┘    └──────────┬───────────────┘│
│         │                               │                │
│         ▼                               ▼                │
│  ┌──────────────────────┐    ┌──────────────────────────┐│
│  │    Event Bus          │    │   Plugin Instance         ││
│  │    (tokio broadcast)  │    │   ┌────────────────────┐ ││
│  └──────┬───────────────┘    │   │ extension.wasm     │ ││
│         │                    │   │ exports:           │ ││
│    ┌────┼────────────┐       │   │  init()            │ ││
│    │    │    │       │       │   │  on_tool_call()    │ ││
│    ▼    ▼    ▼       ▼       │   │  on_tool_result()  │ ││
│  Agent Session  UI   Perm    │   │  on_session_start()│ ││
│                           │   │  ...                │ ││
│                           │   └────────────────────┘ ││
│                           └──────────────────────────┘│
└──────────────────────────────────────────────────────────┘

Plugin API borders (WIT contracts):
  Host exports (plugin calls):
    - tool-registry: register_tool()
    - command-registry: register_command()
    - provider-registry: register_provider()
    - ui: notify(), confirm(), select(), input()
    - session: get_entries_json(), append_entry()
    - host-calls: exec(), http_get(), http_post()

  Plugin exports (host calls):
    - init()                         — 初始化
    - on_session_start()             — 会话启动
    - on_session_shutdown()          — 会话关闭
    - on_agent_start() / on_agent_end()
    - on_turn_start() / on_turn_end()
    - on_tool_call()                 — 可拦截/阻止
    - on_tool_result()               — 可修改结果
    - on_before_agent_start()        — 可注入上下文
    - on_input()                     — 可拦截输入
    - on_context()                   — 可修改消息
```

### 1.2 核心设计决策

| 决策 | 理由 |
|------|------|
| Wasm + wasmtime 运行时 | 安全沙箱、near-native 性能、与 Rust 生态天然契合、无需外部运行时 |
| WIT 接口定义 | 语言无关、编译期类型检查、未来可支持多语言开发插件 |
| 事件 push 模型（host → guest） | 简单高效，zerostack 无需复杂的回调注册系统 |
| 每插件一个 .wasm 实例 | 隔离性好，单个插件崩溃不影响其他插件或宿主 |
| `extension.toml` 清单 | 声明式元数据，统一发现机制 |
| 预编译分发 | 用户无需安装 Rust 工具链，安全且可复现 |

---

## 2. WIT 接口定义

### 2.1 完整 WIT 文件

文件: `crates/plugin-api/wit/plugin-v0.1.0.wit`

```wit
/// zerostack Plugin API v0.1.0
///
/// 这个 WIT 文件定义了插件（guest）与宿主（host）之间的双向接口。
/// Host 为插件提供注册工具/命令/UI 等能力；
/// Guest（插件）导出生命周期回调，由 Host 在适当时机调用。
package zerostack:plugin-api@0.1.0;

// ═══════════════════════════════════════════════
// 基础类型
// ═══════════════════════════════════════════════

interface types {
    /// 工具执行结果
    record tool-output {
        content: string,
        /// JSON 格式的任意元数据 (details)
        details: string,
        /// 执行是否出错
        is-error: bool,
    }

    /// 插件对工具调用的处理决策
    variant tool-action {
        allow,
        block(string),     // 阻止原因
        modify(string),    // 修改后的输入 (JSON)
    }

    /// 插件对 before_agent_start 的处理
    record agent-start-result {
        /// 可选：注入一条自定义消息到会话中
        injected-message: option<string>,
        /// 可选：修改系统 prompt（追加或替换）
        modified-system-prompt: option<string>,
    }

    /// 插件对 input 事件的处理
    variant input-action {
        continue,              // 放行，不做处理
        transform(string),     // 改写输入文本
        handled,               // 已处理，跳过 LLM
    }

    /// UI 通知级别
    variant notify-level {
        info,
        warning,
        error,
        success,
    }

    /// 当前上下文信息
    record context-info {
        /// 工作目录
        cwd: string,
        /// 会话文件路径（可能为空）
        session-file: option<string>,
        /// 会话 ID
        session-id: string,
        /// 运行模式: "tui" | "print" | "acp"
        mode: string,
        /// 是否有 UI（TUI 或 RPC 为 true）
        has-ui: bool,
    }

    /// 工具调用事件
    record tool-call-event {
        tool-name: string,
        tool-call-id: string,
        /// 工具参数 (JSON)
        input-json: string,
        context: context-info,
    }

    /// 工具结果事件
    record tool-result-event {
        tool-name: string,
        tool-call-id: string,
        content: string,
        details-json: string,
        is-error: bool,
    }

    /// 主机命令执行结果
    record exec-output {
        stdout: string,
        stderr: string,
        exit-code: s32,
    }

    /// HTTP 响应
    record http-response {
        status: u16,
        body: string,
    }

    /// 会话条目 (JSON 序列化)
    record session-entry {
        id: string,
        role: string,
        content: string,
        timestamp: u64,
    }
}

// ═══════════════════════════════════════════════
// Host 导出（插件可调用的能力）
// ═══════════════════════════════════════════════

interface tool-registry {
    use types.{tool-output};

    /// 工具定义
    record tool-definition {
        /// 工具名 (snake_case, 如 "my_search")
        name: string,
        /// 显示标签
        label: string,
        /// 描述（给 LLM 看的）
        description: string,
        /// JSON Schema 格式的参数定义
        parameters-schema: string,
        /// 可选：工具能力的一句话描述，出现在 prompt 工具列表中
        prompt-snippet: option<string>,
        /// 可选：工具使用指南，追加到 prompt 的 Guidelines 节
        prompt-guidelines: list<string>,
    }

    /// 注册一个工具。注册后，当 LLM 调用该工具时，
    /// Host 会调用插件的 `on-tool-execute` 导出函数。
    register-tool: func(def: tool-definition) -> result<_, string>;

    /// 取消注册一个工具
    unregister-tool: func(name: string) -> result<_, string>;
}

interface command-registry {
    /// 命令定义
    record command-definition {
        /// 命令名（不含 /, 如 "my-command"）
        name: string,
        /// 描述
        description: string,
        /// 可选：参数提示 (如 "<branch-name>")
        argument-hint: option<string>,
    }

    /// 注册一个斜杠命令。当用户输入 `/name` 时，
    /// Host 调用插件的 `on-command` 导出函数。
    register-command: func(def: command-definition) -> result<_, string>;

    /// 取消注册一个命令
    unregister-command: func(name: string) -> result<_, string>;
}

interface provider-registry {
    /// 模型定义
    record provider-model {
        id: string,
        name: string,
        reasoning: bool,
        /// 输入类型: "text" | "image"
        input-types: list<string>,
        cost-input: float32,
        cost-output: float32,
        cost-cache-read: float32,
        cost-cache-write: float32,
        context-window: u32,
        max-tokens: u32,
    }

    /// 提供者配置
    record provider-config {
        /// 显示名
        name: string,
        /// API 端点
        base-url: string,
        /// API Key（字面量或 $ENV_VAR 占位符）
        api-key: string,
        /// API 类型: "openai-completions" | "anthropic-messages" | ...
        api-type: string,
        /// 是否添加 Authorization: Bearer 头
        auth-header: bool,
        /// 模型列表
        models: list<provider-model>,
    }

    register-provider: func(id: string, config: provider-config) -> result<_, string>;
    unregister-provider: func(id: string) -> result<_, string>;
}

interface ui {
    use types.{notify-level};

    /// 显示通知
    notify: func(level: notify-level, message: string);

    /// 请求用户确认 (yes/no)
    /// 返回 false 表示用户取消
    confirm: func(title: string, message: string) -> result<bool, string>;

    /// 让用户从选项中选择
    /// 返回选项索引，或 none 表示取消
    select: func(title: string, options: list<string>) -> result<option<u32>, string>;

    /// 提示用户输入文本
    /// 返回输入内容，或 none 表示取消
    input: func(prompt: string) -> result<option<string>, string>;

    /// 设置状态栏文本
    set-status: func(key: string, text: string);

    /// 清除状态栏文本
    clear-status: func(key: string);
}

interface session {
    use types.{session-entry};

    /// 获取所有会话条目（JSON 数组）
    get-entries: func() -> result<list<session-entry>, string>;

    /// 获取当前叶子节点 ID
    get-leaf-id: func() -> result<string, string>;

    /// 追加一条自定义消息到会话中
    append-entry: func(
        custom-type: string,
        content: string,
        details: string,
    ) -> result<_, string>;

    /// 获取系统 prompt（当前会话使用的）
    get-system-prompt: func() -> result<string, string>;
}

interface host-calls {
    use types.{exec-output, http-response};

    /// 执行命令（受权限系统控制）
    exec: func(cmd: string, args: list<string>) -> result<exec-output, string>;

    /// HTTP GET 请求
    http-get: func(url: string) -> result<http-response, string>;

    /// HTTP POST 请求
    http-post: func(url: string, body: string) -> result<http-response, string>;
}

// ═══════════════════════════════════════════════
// 事件订阅（插件告诉 host 它关心哪些事件）
// ═══════════════════════════════════════════════

interface event-subscription {
    /// 插件在 init() 期间调用此函数来声明它处理哪些事件。
    /// 未声明的事件不会被路由到插件，节省调用开销。
    subscribe: func(event-names: list<string>);
}

// ═══════════════════════════════════════════════
// World: 定义完整的插件契约
// ═══════════════════════════════════════════════

world extension {
    // Host 提供给插件的接口
    import event-subscription;
    import tool-registry;
    import command-registry;
    import provider-registry;
    import ui;
    import session;
    import host-calls;

    // 插件必须导出的函数
    // ═══════════════════════════════════════

    /// 插件初始化。在加载时调用一次。
    /// 插件应在此处调用 register_tool/on 等方法注册自身能力。
    export init: func() -> result<_, string>;

    /// 生命周期事件处理器（仅在 subscribe 声明后才会被调用）
    export on-session-start: func(reason: string) -> result<_, string>;
    export on-session-shutdown: func(reason: string) -> result<_, string>;
    export on-agent-start: func() -> result<_, string>;
    export on-agent-end: func() -> result<_, string>;
    export on-turn-start: func(turn-index: u32) -> result<_, string>;
    export on-turn-end: func(turn-index: u32) -> result<_, string>;

    /// before_agent_start: 可注入消息或修改 system prompt
    export on-before-agent-start: func(
        prompt: string,
        system-prompt: string,
    ) -> result<option<types.agent-start-result>, string>;

    /// 工具调用拦截。返回 tool-action 来控制行为。
    /// 返回 none = 放行（allow）。
    export on-tool-call: func(
        event: types.tool-call-event,
    ) -> result<option<types.tool-action>, string>;

    /// 工具结果修改。返回修改后的 tool-output，或 none 保持原样。
    export on-tool-result: func(
        event: types.tool-result-event,
    ) -> result<option<types.tool-output>, string>;

    /// 用户输入拦截
    export on-input: func(
        text: string,
        source: string,    // "interactive" | "rpc" | "extension"
    ) -> result<option<types.input-action>, string>;

    /// 上下文修改（每次 LLM 调用前）。返回修改后的 messages JSON。
    export on-context: func(
        messages-json: string,
    ) -> result<option<string>, string>;

    /// 命令处理。当用户输入 `/name` 且插件注册了该命令时调用。
    export on-command: func(
        name: string,
        args: string,
    ) -> result<_, string>;

    /// 工具执行。当 LLM 调用插件注册的工具时调用。
    export on-tool-execute: func(
        tool-name: string,
        tool-call-id: string,
        params-json: string,
    ) -> result<types.tool-output, string>;
}
```

### 2.2 WIT 到 Rust 的代码生成

通过 `wit-bindgen` 自动生成 Rust trait：

```rust
// 生成的 trait（概念示意）：
#[async_trait]
pub trait Extension {
    async fn init(&mut self, ctx: ExtensionContext) -> Result<(), String>;

    async fn on_session_start(&mut self, reason: String) -> Result<(), String>;
    async fn on_session_shutdown(&mut self, reason: String) -> Result<(), String>;
    async fn on_agent_start(&mut self) -> Result<(), String>;
    async fn on_agent_end(&mut self) -> Result<(), String>;
    async fn on_turn_start(&mut self, turn_index: u32) -> Result<(), String>;
    async fn on_turn_end(&mut self, turn_index: u32) -> Result<(), String>;
    async fn on_before_agent_start(&mut self, prompt: String, system_prompt: String) -> Result<Option<AgentStartResult>, String>;
    async fn on_tool_call(&mut self, event: ToolCallEvent) -> Result<Option<ToolAction>, String>;
    async fn on_tool_result(&mut self, event: ToolResultEvent) -> Result<Option<ToolOutput>, String>;
    async fn on_input(&mut self, text: String, source: String) -> Result<Option<InputAction>, String>;
    async fn on_context(&mut self, messages_json: String) -> Result<Option<String>, String>;
    async fn on_command(&mut self, name: String, args: String) -> Result<(), String>;
    async fn on_tool_execute(&mut self, tool_name: String, tool_call_id: String, params_json: String) -> Result<ToolOutput, String>;
}
```

`ExtensionContext` 提供所有 host-exported 接口的调用方法：
```rust
impl ExtensionContext {
    // event-subscription
    fn subscribe(&self, events: &[&str]);
    // tool-registry
    fn register_tool(&self, def: ToolDefinition) -> Result<(), String>;
    fn unregister_tool(&self, name: &str) -> Result<(), String>;
    // command-registry
    fn register_command(&self, def: CommandDefinition) -> Result<(), String>;
    // provider-registry
    fn register_provider(&self, id: &str, config: ProviderConfig) -> Result<(), String>;
    // ui
    fn notify(&self, level: NotifyLevel, message: &str);
    fn confirm(&self, title: &str, message: &str) -> Result<bool, String>;
    async fn select(&self, title: &str, options: &[String]) -> Result<Option<u32>, String>;
    async fn input(&self, prompt: &str) -> Result<Option<String>, String>;
    fn set_status(&self, key: &str, text: &str);
    fn clear_status(&self, key: &str);
    // session
    fn get_entries(&self) -> Result<Vec<SessionEntry>, String>;
    fn get_leaf_id(&self) -> Result<String, String>;
    fn append_entry(&self, custom_type: &str, content: &str, details: &str) -> Result<(), String>;
    // host-calls
    async fn exec(&self, cmd: &str, args: &[String]) -> Result<ExecOutput, String>;
    async fn http_get(&self, url: &str) -> Result<HttpResponse, String>;
    async fn http_post(&self, url: &str, body: &str) -> Result<HttpResponse, String>;
}
```

---

## 3. 插件清单 — extension.toml

### 3.1 完整格式

```toml
# extension.toml — zerostack 插件清单
# 放在插件仓库根目录，作为插件身份和元数据的唯一来源。

# --- 必需字段 ---

# 全局唯一 ID，格式: <namespace>/<name>
# 命名空间: 个人 GitHub 用户名，或组织名（如 "zerostack-community"）
id = "sternelee/my-protected-paths"

# 显示名
name = "Protected Paths"

# 语义化版本
version = "0.1.0"

# 清单格式版本（当前为 1）
schema_version = 1

# 作者列表
authors = ["Your Name <email@example.com>"]

# 一句话描述
description = "Block write/edit operations on sensitive paths like .env and node_modules/"

# 源码仓库 URL
repository = "https://github.com/sternelee/zerostack-protected-paths"


# --- 可选字段 ---

# 许可证
license = "MIT"

# 主页
homepage = "https://github.com/sternelee/zerostack-protected-paths#readme"

# 图标（相对于仓库根目录）
icon = "assets/icon.png"

# 关键词（用于搜索）
keywords = ["security", "permissions", "protection"]


# --- 插件配置 ---

[plugin]
# Wasm 入口文件（相对于仓库根目录，CI 构建产物）
entrypoint = "target/wasm32-wasip2/release/plugin.wasm"

# 所需的最小 zerostack 版本
minimum_zerostack_version = "1.6.0"


# --- 能力声明（声明性，影响权限提示） ---

[capabilities]
# 是否注册自定义工具
tools = true
# 是否注册斜杠命令
commands = false
# 是否监听生命周期事件
lifecycle = true
# 是否注册 LLM provider
provider = false
# 是否调用 UI 交互
ui = true
# 是否需要执行 shell 命令
exec = false
# 是否需要 HTTP 请求
http = true
# 是否需要访问会话数据
session = false
```

### 3.2 能力声明的安全意义

能力声明影响用户安装时的权限提示：

```bash
$ zerostack plugin install git:github.com/sternelee/my-plugin@v0.1.0

Plugin: my-plugin v0.1.0
Capabilities:
  ✓ tools         — Can register custom tools
  ✗ commands      — No slash commands
  ✓ lifecycle     — Can observe session/agent events
  ✓ ui            — Can show notifications & prompts
  ✗ exec          — No shell access
  ✓ http          — Can make HTTP requests
  ✗ session       — No session data access

Install? [y/N]
```

---

## 4. 插件运行时架构

### 4.1 PluginHost

```rust
// src/plugin/host.rs

use wasmtime::{Engine, Module, Store, Linker, Config};
use std::collections::HashMap;
use std::path::PathBuf;

/// 管理 wasmtime Engine 和所有已加载的插件实例。
pub struct PluginHost {
    /// 共享的 wasmtime Engine（编译缓存、类型系统）
    engine: Engine,
    /// 已加载的插件实例
    instances: HashMap<String, PluginInstance>,
    /// 共享的 Host 状态（通过 WIT imports 暴露给插件）
    host_state: Arc<HostState>,
}

/// 单个插件实例
struct PluginInstance {
    /// wasmtime Store（包含 guest 内存）
    store: Store<GuestState>,
    /// 编译后的模块
    module: Module,
    /// 插件元数据
    manifest: PluginManifest,
    /// 已订阅的事件列表
    subscriptions: Vec<String>,
    /// 已注册的工具名
    registered_tools: Vec<String>,
    /// 已注册的命令名
    registered_commands: Vec<String>,
}
```

### 4.2 加载流程

```
1. 发现插件
   ├── 扫描 ~/.local/share/zerostack/plugins/  (全局)
   └── 扫描 .zerostack/plugins/  (项目级)

2. 解析 extension.toml
   │
3. 验证能力声明 & 版本兼容性
   │
4. 加载 .wasm 到 wasmtime Module (编译 & 缓存)
   │
5. 实例化 Store + 注入 Host imports
   │
6. 调用 guest export: init()
   │   └── 插件在此调用 register_tool / on / register_command 等
   │
7. 记录注册信息到 PluginInstance
   │
8. 插件就绪，等待事件
```

### 4.3 事件路由

```rust
impl PluginHost {
    /// 向所有订阅了该事件的插件发送事件
    pub async fn dispatch_event(&self, event: PluginEvent) {
        for (id, instance) in &self.instances {
            if !instance.subscriptions.contains(&event.name()) {
                continue;
            }
            // 在 wasmtime Store 中调用 guest export
            instance.call_export(&event).await;
        }
    }
}

enum PluginEvent {
    SessionStart { reason: String },
    SessionShutdown { reason: String },
    AgentStart,
    AgentEnd,
    TurnStart { turn_index: u32 },
    TurnEnd { turn_index: u32 },
    ToolCall(ToolCallEvent),
    ToolResult(ToolResultEvent),
    BeforeAgentStart { prompt: String, system_prompt: String },
    Input { text: String, source: String },
    Context { messages_json: String },
}
```

### 4.4 超时与隔离

```rust
// 每个 guest 调用都有超时限制
const GUEST_CALL_TIMEOUT: Duration = Duration::from_secs(5);

// 内存限制
const GUEST_MEMORY_LIMIT: usize = 16 * 1024 * 1024; // 16 MB

// wasmtime Config
fn engine_config() -> Config {
    let mut config = Config::default();
    config.wasm_component_model(true);  // 启用 WIT component model
    config.epoch_interruption(true);    // 支持超时中断
    config.consume_fuel(true);          // 燃料计量（防无限循环）
    config
}
```

---

## 5. 事件生命周期

### 5.1 完整生命周期图

```
zerostack 启动
  │
  ├─► PluginHost::load_all() — 加载所有已安装插件
  │     └── 每个插件: init() → subscribe() / register_tool() / register_command()
  │
  ├─► on-session-start(reason: "startup")
  │
  ▼
用户输入文本
  │
  ├─► on-input(text, source) → Continue / Transform / Handled
  │     └── Handled: 跳过 LLM 处理
  │
  ├─► on-before-agent-start(prompt, system-prompt)
  │     └── 可注入消息、修改 system prompt
  │
  ├─► on-agent-start()
  │
  ┌─── Agent Turn Loop ────────────────────────────────────┐
  │                                                        │
  │  on-turn-start(turn_index)                              │
  │                                                        │
  │  on-context(messages_json)                              │
  │    └── 可过滤/修改消息列表                                │
  │                                                        │
  │  LLM 响应...                                           │
  │                                                        │
  │  当 LLM 调用工具时:                                      │
  │  ├── on-tool-call(event) → Allow / Block / Modify      │
  │  │     └── Block: 跳过执行                               │
  │  │     └── Modify: 修改 input-json 后执行                │
  │  │                                                      │
  │  ├── [工具执行]                                         │
  │  │     ├── 内置工具: 正常执行                            │
  │  │     └── 插件工具: on-tool-execute(name, params)      │
  │  │                                                      │
  │  └── on-tool-result(event) → Option<modified_output>   │
  │        └── 可修改工具输出内容                             │
  │                                                        │
  │  on-turn-end(turn_index)                                │
  │                                                        │
  └───────────────────────────────────────────────────────┘
  │
  ├─► on-agent-end()
  │
  ▼
用户继续输入... (重复上述流程)

/new 或 /resume 或 /fork:
  ├─► on-session-shutdown(reason)
  ├─► on-session-start(reason: "new" | "resume" | "fork")

退出 (Ctrl+C / /quit):
  └─► on-session-shutdown(reason: "quit")
```

### 5.2 事件处理返回值

| 事件 | 返回值 | 行为 |
|------|--------|------|
| `on-tool-call` | `none` / `allow` | 放行 |
| | `block(reason)` | 阻止执行，reason 返回给 LLM |
| | `modify(new_json)` | 替换输入参数后执行 |
| `on-tool-result` | `none` | 保持原结果 |
| | `tool-output` | 替换结果 |
| `on-before-agent-start` | `none` | 不做修改 |
| | `agent-start-result` | 注入消息 / 修改 system prompt |
| `on-input` | `none` / `continue` | 正常处理 |
| | `transform(text)` | 改写输入 |
| | `handled` | 跳过 LLM |
| `on-context` | `none` | 不修改消息 |
| | `messages-json` | 替换消息列表 |
| 其他生命周期 | 忽略返回值 | — |

---

## 6. 集成点设计

### 6.1 与 Agent Builder 集成

```rust
// src/agent/builder.rs — 修改后的 build_agent_inner()

pub async fn build_agent_inner<M: CompletionModel + 'static>(
    model: M,
    cli: &Cli,
    cfg: &Config,
    context: &ContextFiles,
    permission: Option<PermCheck>,
    ask_tx: Option<AskSender>,
    sandbox: Sandbox,
    reasoning_enabled: bool,
    temperature: Option<f64>,
    additional_params: Option<serde_json::Value>,
    #[cfg(feature = "mcp")] mcp_manager: Option<&McpClientManager>,
    // 新增: 插件注册的工具
    plugin_tools: Vec<Box<dyn rig::tool::ToolDyn>>,
) -> Agent<M> {
    // ... preamble ...

    let mut builder = AgentBuilder::new(model)
        .preamble(&preamble);
    // ...

    // 内置工具
    let mut all_tools: Vec<Box<dyn rig::tool::ToolDyn>> = vec![
        Box::new(tools::ReadTool::new(...)),
        Box::new(tools::WriteTool::new(...)),
        // ...
    ];

    // 插件工具（追加到最后）
    all_tools.extend(plugin_tools);

    builder = builder.tools(all_tools);
    // ...
}
```

### 6.2 与 Agent Runner 集成

```rust
// src/agent/runner.rs — 修改后的 spawn_agent()

pub fn spawn_agent<M, P>(
    agent: Agent<M, P>,
    // ... 现有参数 ...
    plugin_host: Arc<PluginHost>,       // 新增
) -> AgentRunner {
    // ... 现有流处理逻辑 ...

    // 在 LLM 流中插入插件事件
    while let Some(item) = stream.next().await {
        match item {
            MultiTurnStreamItem::ToolInput { tool_call } => {
                // 1. 构建事件
                let event = PluginEvent::ToolCall(ToolCallEvent { ... });
                // 2. 分发给插件
                let action = plugin_host.dispatch_tool_call(&event).await;
                // 3. 根据返回值决定行为
                match action {
                    ToolAction::Allow => { /* 继续执行 */ }
                    ToolAction::Block(reason) => {
                        // 返回 block 结果给 LLM
                        event_tx.send(AgentEvent::ToolResult {
                            name: "blocked".into(),
                            output: reason.into(),
                        }).await;
                        continue;
                    }
                    ToolAction::Modify(new_input) => {
                        // 替换输入参数
                        tool_call.arguments = new_input;
                    }
                }
            }
            // ... 其他事件 ...
        }
    }
}
```

### 6.3 与 UI / Slash Commands 集成

```rust
// src/ui/slash/mod.rs — handle_slash() 函数修改

pub async fn handle_slash(
    input: &str,
    // ... 现有参数 ...
    plugin_host: &PluginHost,  // 新增
) -> Option<SlashResult> {
    let (cmd, args) = parse_slash(input)?;

    // 1. 先检查内置命令
    if let Some(result) = handle_builtin_slash(cmd, args).await {
        return Some(result);
    }

    // 2. 再检查插件命令
    if let Some(result) = plugin_host.dispatch_command(cmd, args).await {
        return Some(result);
    }

    None
}
```

### 6.4 与 Permission System 集成

插件工具使用与内置工具相同的权限模型：

```rust
// 插件工具通过一个 wrapper 接入权限系统：
pub struct PluginToolWrapper {
    inner_tool_name: String,
    permission: Option<PermCheck>,
    ask_tx: Option<AskSender>,
    plugin_host: Arc<PluginHost>,
}

impl rig::tool::ToolDyn for PluginToolWrapper {
    fn name(&self) -> &str { &self.inner_tool_name }
    // ... 委托给 plugin_host.execute_tool() ...
}
```

`plugin_host.execute_tool()` → 调用 guest 的 `on-tool-execute` export。

---

## 7. 打包与分发

### 7.1 目录结构（插件仓库）

```
my-plugin/
├── extension.toml          # 清单
├── src/
│   └── lib.rs              # 插件 Rust 代码
├── Cargo.toml              # Rust 项目配置
├── .github/
│   └── workflows/
│       └── build.yml       # CI: 编译 wasm + 发布
├── assets/
│   └── icon.png
└── README.md
```

### 7.2 Cargo.toml 模板

```toml
[package]
name = "zerostack-plugin-my-tool"
version = "0.1.0"
edition = "2024"

[lib]
crate-type = ["cdylib"]

[dependencies]
# 由 zerostack 提供的 plugin API crate
zerostack-plugin-api = "0.1"
# WIT bindings
wit-bindgen = "0.42"

# 其他依赖（将编译进 wasm）
serde = { version = "1", features = ["derive"] }
serde_json = "1"

[profile.release]
opt-level = "s"     # 优化体积
lto = true
strip = true
```

### 7.3 插件代码示例

```rust
// src/lib.rs — Protected Paths 插件

use zerostack_plugin_api::*;

struct ProtectedPathsPlugin {
    protected: Vec<String>,
}

impl Extension for ProtectedPathsPlugin {
    async fn init(&mut self, ctx: ExtensionContext) -> Result<(), String> {
        // 声明关心的事件
        ctx.subscribe(&["tool-call"]);

        // 声明能力（用于安全提示）
        ctx.set_capabilities(Capabilities {
            tools: false,
            commands: false,
            lifecycle: true,
            ..Default::default()
        });

        Ok(())
    }

    async fn on_tool_call(
        &mut self,
        event: ToolCallEvent,
    ) -> Result<Option<ToolAction>, String> {
        if event.tool_name != "write" && event.tool_name != "edit" {
            return Ok(None); // 不关心的工具，放行
        }

        let path = extract_path(&event.input_json);
        let is_protected = self.protected.iter().any(|p| path.contains(p));

        if is_protected {
            Ok(Some(ToolAction::Block(
                format!("Path \"{}\" is protected by Protected Paths plugin", path)
            )))
        } else {
            Ok(None) // 放行
        }
    }

    // 其他未使用的事件可以不实现（有默认空实现）
}
```

### 7.4 CI 构建流水线

```yaml
# .github/workflows/build.yml
name: Build and Release Plugin

on:
  push:
    tags: ['v*']

jobs:
  build:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Install Rust
        uses: dtolnay/rust-toolchain@stable
        with:
          targets: wasm32-wasip2

      - name: Build
        run: cargo build --release --target wasm32-wasip2

      - name: Package
        run: |
          mkdir -p dist
          cp target/wasm32-wasip2/release/*.wasm dist/plugin.wasm
          cp extension.toml dist/
          cp -r assets dist/ 2>/dev/null || true
          cp README.md dist/ 2>/dev/null || true
          tar -czf plugin.tar.gz -C dist .

      - name: Create Release
        uses: softprops/action-gh-release@v2
        with:
          files: plugin.tar.gz
```

### 7.5 Registry 格式 — extensions.toml

```toml
# 在 zerostack-contrib/extensions 仓库中

[plugins.sternelee-protected-paths]
id = "sternelee/protected-paths"
name = "Protected Paths"
description = "Block write/edit on .env, node_modules/, .git/ and more"
repository = "https://github.com/sternelee/zerostack-protected-paths"
version = "0.1.0"
keywords = ["security", "permissions"]

[plugins.sternelee-git-checkpoint]
id = "sternelee/git-checkpoint"
name = "Git Checkpoint"
description = "Auto-stash on each turn, restore on fork"
repository = "https://github.com/sternelee/zerostack-git-checkpoint"
version = "0.1.0"
keywords = ["git", "safety", "undo"]

# ... more plugins ...
```

### 7.6 分发流程

```
开发者                    注册中心(extensions.toml)          用户
  │                            │                            │
  │ 1. 创建插件仓库              │                            │
  │    extension.toml           │                            │
  │    src/lib.rs               │                            │
  │    CI workflow              │                            │
  │                            │                            │
  │ 2. git tag v0.1.0          │                            │
  │    → CI 构建 wasm + pack    │                            │
  │    → GitHub Release        │                            │
  │                            │                            │
  │ 3. PR 到 extensions.toml   │                            │
  │    → 添加插件条目           │                            │
  │                            │                            │
  │                            │ 4. CI 验证                  │
  │                            │    解析 extension.toml ✓    │
  │                            │    下载 wasm ✓              │
  │                            │    检查能力声明 ✓            │
  │                            │    合并到 main              │
  │                            │                            │
  │                            │                            │ 5. zerostack plugin search
  │                            │                            │    → 获取 extensions.toml
  │                            │                            │    → 展示可用插件列表
  │                            │                            │
  │                            │                            │ 6. zerostack plugin install
  │                            │                            │    → 从 GitHub Release 下载
  │                            │                            │    → 解压到 plugins 目录
  │                            │                            │    → 验证签名/校验和
  │                            │                            │    → 加载 PluginHost
```

---

## 8. CLI 命令

### 8.1 插件管理命令

```bash
# 搜索插件
zerostack plugin search <query>
zerostack plugin search security

# 安装插件
zerostack plugin install git:github.com/sternelee/my-plugin@v0.1.0
zerostack plugin install file:./my-local-plugin
zerostack plugin install npm:@scope/package    # 未来支持

# 全局 vs 项目级
zerostack plugin install --local git:github.com/...
# --local: 安装到 .zerostack/plugins/ (项目级)
# 默认: 安装到 ~/.local/share/zerostack/plugins/ (全局)

# 列出已安装插件
zerostack plugin list
# 输出:
#   Name              Version  Capabilities
#   protected-paths   0.1.0    lifecycle, ui
#   git-checkpoint    0.1.0    lifecycle, exec

# 移除插件
zerostack plugin remove sternelee/protected-paths

# 更新插件
zerostack plugin update sternelee/protected-paths

# 禁用/启用插件
zerostack plugin disable sternelee/protected-paths
zerostack plugin enable sternelee/protected-paths

# 查看插件详情
zerostack plugin info sternelee/protected-paths
# 输出完整 extension.toml 内容 + 运行时状态
```

### 8.2 斜杠命令（TUI 内）

在 TUI 内，除了 CLI 命令，还提供斜杠命令：

```
/plugin list                    — 列出已加载的插件
/plugin install <source>       — 安装插件
/plugin remove <id>            — 移除插件
/plugin toggle <id>            — 切换启用/禁用
```

---

## 9. 实现路线

### Phase 1: 最小可行内核 (MVP)

**目标：** 加载一个预编译的 `.wasm` 插件，注册一个自定义工具，让 LLM 能调用它。

**任务清单：**

1. **创建 `crates/plugin-api/` crate**
   - 编写完整 WIT 文件（`plugin-v0.1.0.wit`）
   - 配置 `wit-bindgen` 代码生成
   - 定义 Rust 侧的 `Extension` trait 和 `ExtensionContext`
   - 导出 `zerostack-plugin-api` crate 供插件作者使用

2. **创建 `src/plugin/` 模块** (feature-gated: `feature = "plugins"`)
   - `src/plugin/host.rs` — `PluginHost`: 管理 wasmtime Engine + 实例生命周期
   - `src/plugin/manager.rs` — `PluginManager`: 发现、加载、卸载插件
   - `src/plugin/loader.rs` — 文件系统扫描 + `extension.toml` 解析
   - `src/plugin/wit_imports.rs` — 实现 WIT imports（tool-registry, ui, host-calls 等）
   - `src/plugin/wrapper.rs` — `PluginToolWrapper`（将 guest 工具适配为 `rig::tool::ToolDyn`）

3. **集成到 Agent Builder**
   - 修改 `build_agent_inner()` 接收 `plugin_tools`
   - 将插件工具追加到工具列表

4. **基本测试框架**
   - 编写一个最小 wasm 测试插件
   - 在测试中加载插件并调用其工具

**交付物：** 可以通过 `zerostack --plugin ./my-plugin.wasm` 加载一个工具插件。

### Phase 2: 事件系统

**目标：** 完整的 12 个生命周期事件 + 拦截/修改能力。

**任务清单：**

1. **实现事件分发**
   - `PluginHost::dispatch_event()` — 遍历所有实例，调用对应的 guest export
   - 事件队列：确保工具调用事件按顺序处理（尤其是 block 行为）

2. **集成到 Agent Runner**
   - `spawn_agent()` — 在关键节点插入事件分发
   - `handle_agent_event()` — 修改工具调用流程以支持 block/modify

3. **集成到 UI 层**
   - `handle_slash()` — 插件命令路由
   - `InputEditor` — `on-input` 事件分发

4. **上下文访问**
   - `session` 接口实现（get_entries, append_entry）
   - `on-context` 事件（消息修改）
   - `on-before-agent-start` 事件（注入消息 / 修改 system prompt）

**交付物：** 插件可以订阅任意事件并做出响应。

### Phase 3: 分发 & CLI

**目标：** 完整的插件生命周期管理。

**任务清单：**

1. **CLI 命令**
   - `zerostack plugin install|remove|list|update|search|info`
   - `--local` 标志支持项目级安装
   - 能力审核提示

2. **Registry**
   - `extension.toml` 完整规范定稿
   - `zerostack-contrib/extensions` 仓库 + `extensions.toml`
   - CI 验证流水线

3. **打包工具**
   - `zerostack plugin pack` — 本地打包 .wasm + extension.toml + assets
   - 为插件作者提供模板仓库

4. **安装时安全**
   - 能力声明审计
   - 可选：wasm 签名验证
   - 权限系统集成（插件工具的权限规则）

**交付物：** 完整的插件生态系统基础设施。

### Phase 4: 高级特性

- **Provider 注册** — `register_provider()` 实现
- **自定义 UI** — `ctx.ui.custom()` 在 TUI 中渲染自定义组件
- **多语言支持** — 通过 WIT 的跨语言特性支持 Python/C/Go 编写插件
- **插件依赖** — 插件间依赖与共享
- **热加载** — TUI 内 `/reload` 支持重载插件

---

## 10. 附录：与现有 extras 模块的关系

| 现有 extras 模块 | 与插件系统的关系 | 说明 |
|---|---|---|
| `mcp/` | 互补 | 插件可作为一种分发 MCP 客户端的方式。也可通过插件注册额外的 MCP 服务器连接。 |
| `subagents/` | 互补 | 插件可定义子代理策略。`on_tool_call` 可拦截子代理调用。 |
| `memory/` | 互补 | 插件可通过 `session` 接口读写记忆。未来可注册自定义记忆后端。 |
| `loop/` | 集成 | 循环的每次迭代应触发相应事件（`on_agent_start/end` 等）。 |
| `git_worktree/` | 互补 | `/worktree` 切换应触发 `on-session-shutdown` + `on-session-start`。 |
| `archmd/` | 互补 | 插件可注册额外的上下文加载器（类似 `ARCHITECTURE.md`）。 |
| `advisor/` | 可能替代 | 顾问模式可以考虑迁移为内置插件。 |
| `permission/` | 深度集成 | `on_tool_call` 是权限系统的扩展点。插件权限门与配置权限门共存。 |

---

## 设计总结

| 维度 | 方案 |
|------|------|
| **运行时** | wasmtime，`wasm32-wasip2` 目标 |
| **接口定义** | WIT (Wasm Interface Type) + wit-bindgen |
| **插件语言** | Rust（通过 WIT 未来可扩展其他语言） |
| **事件模型** | Push 模型（host → guest export 调用） |
| **API 风格** | 参考 pi ExtensionAPI（事件驱动 + 注册模式） |
| **分发模型** | 参考 Zed（预编译 .wasm + extension.toml + registry） |
| **安全性** | Wasm 沙箱 + 燃料计量 + 超时 + 能力声明审核 |
| **加载机制** | Feature-gated (`plugins`)，启动时静态加载 |
| **作用域** | 全局 (`~/.local/share/zerostack/plugins/`) + 项目 (`.zerostack/plugins/`) |
