# zerostack GUI 设计文档

**日期**: 2026-07-10
**状态**: 设计完成，待评审

## 1. 概述

为 zerostack 开发基于 [Makepad](https://github.com/makepad/makepad) 框架的 GUI 窗口程序。GUI 与现有 crossterm TUI 并存，共享核心引擎，通过 `--gui` CLI 参数切换。

### 目标

- TUI 和 GUI 两种前端，共享同一套核心逻辑
- 聊天面板式 GUI 布局（类似 ChatGPT / Claude Desktop）
- 完整功能对等：流式聊天、Markdown 渲染、代码高亮、工具调用、权限弹窗、会话管理、MCP、扩展、子代理、记忆
- 先提取核心引擎，后开发 GUI

### 非目标

- 不替代 TUI，两者并存
- 不重构现有 TUI 代码（`src/` 目录保持不变）
- 不改变现有 feature flag 体系

---

## 2. 架构方案

采用 **Channel-based 事件架构**（方案 A）。

### 核心思路

```
┌──────────────────────────────────────────────────────────────┐
│                        CoreEngine                             │
│  (tokio runtime, config, agent, tools, sessions,             │
│   permissions, extensions, mcp, subagents, memory)           │
│                                                               │
│  ┌──────────┐  CoreEvent    ┌──────────────┐                │
│  │  Core    │──────────────▶│  Frontend     │                │
│  │  Engine  │◀──────────────│  (TUI/GUI)    │                │
│  └──────────┘  UserAction   └──────────────┘                │
│       ▲                         │                             │
│       │    tokio::mpsc          │                             │
│       └─────────────────────────┘                             │
└──────────────────────────────────────────────────────────────┘
```

- `CoreEngine` 是纯数据结构，不依赖任何 UI 框架
- 与前端通过 `tokio::mpsc::unbounded_channel` 通信
- `CoreEvent` 从核心推送到前端，`UserAction` 从前端发送到核心

---

## 3. 项目结构

```
zerostack/
├── crates/
│   ├── core/                    # ★ 核心引擎（新 crate）
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── engine.rs        # CoreEngine 主结构体
│   │   │   ├── events.rs        # CoreEvent + UserAction 枚举
│   │   │   ├── agent/           # ← 从 src/agent/ 迁移
│   │   │   ├── config/          # ← 从 src/config/ 迁移
│   │   │   ├── session/         # ← 从 src/session/ 迁移
│   │   │   ├── provider.rs      # ← 从 src/ 迁移
│   │   │   ├── permission/      # ← 从 src/permission/ 迁移
│   │   │   ├── extension/       # ← 从 src/extension/ 迁移
│   │   │   ├── extras/          # ← 从 src/extras/ 迁移
│   │   │   ├── fs.rs            # ← 从 src/ 迁移
│   │   │   ├── auth.rs          # ← 从 src/ 迁移
│   │   │   └── ...
│   │   └── Cargo.toml           # 不依赖 crossterm/makepad
│   │
│   ├── gui/                     # ★ Makepad GUI（新 crate）
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── app.rs           # MakepadApp: 持有 GuiBridge
│   │   │   ├── bridge.rs        # tokio ↔ Makepad 桥接
│   │   │   ├── views/
│   │   │   │   ├── mod.rs
│   │   │   │   ├── main_view.rs # 主布局
│   │   │   │   ├── sidebar.rs   # 会话列表
│   │   │   │   ├── chat_view.rs # 聊天区域
│   │   │   │   └── input_bar.rs # 输入框
│   │   │   ├── components/
│   │   │   │   ├── mod.rs
│   │   │   │   ├── message_bubble.rs
│   │   │   │   ├── tool_card.rs
│   │   │   │   └── code_block.rs
│   │   │   └── theme.rs
│   │   └── Cargo.toml           # 依赖 zerostack-core + makepad
│   │
│   └── extension-api/           # 保持不变
│
├── src/                         # 现有 TUI 代码保持不变
│   ├── main.rs
│   ├── ui/
│   └── ...
│
├── Cargo.toml                   # Workspace 根
└── tests/
```

### 命名

- 新 crates 命名为 `core` 和 `gui`（简洁命名）
- 现有 `src/` 中的 TUI 代码不移动，不重构

---

## 4. 核心引擎设计

### 4.1 CoreEngine

```rust
pub struct CoreEngine {
    config: Config,
    sessions: SessionManager,
    permission: PermissionChecker,
    agent_runner: Option<AgentRunner>,
    // MCP, extensions, subagents, memory 等
}

impl CoreEngine {
    /// 创建引擎
    pub fn new(config: Config) -> Self;

    /// 处理前端发来的动作，返回响应事件列表
    pub async fn handle_action(&mut self, action: UserAction) -> Vec<CoreEvent>;

    /// 获取初始状态（会话列表、配置等），前端启动时调用
    pub fn initial_state(&self) -> InitialState;
}
```

### 4.2 事件系统

#### CoreEvent（核心 → 前端）

```rust
pub enum CoreEvent {
    // === 流式输出 ===
    StreamingDelta { text: CompactString },
    ReasoningDelta { text: CompactString },
    CompletionCall {
        input_tokens: u64,
        output_tokens: u64,
        cached_input_tokens: u64,
        cache_creation_input_tokens: u64,
    },

    // === 工具调用 ===
    ToolCall { name: CompactString, args: serde_json::Value },
    ToolResult { name: CompactString, output: CompactString },
    SubagentToolCall { name: CompactString, args: serde_json::Value },

    // === 权限 ===
    PermissionNeeded { id: u64, tool_name: CompactString, args: String },

    // === 消息生命周期 ===
    MessageComplete { response: CompactString, tokens: TokenUsage },
    Retrying { attempt: usize, max: usize },

    // === 会话管理 ===
    SessionListUpdated { sessions: Vec<SessionInfo> },
    SessionChanged { session_id: CompactString },

    // === 状态 ===
    StatusUpdate { model: CompactString, tokens_used: u64, mode: SecurityMode },
    ConfigChanged,

    // === 系统 ===
    Error { message: CompactString },
}
```

#### UserAction（前端 → 核心）

```rust
pub enum UserAction {
    // === 消息 ===
    SendMessage { text: CompactString },
    CancelStream,

    // === 权限 ===
    PermissionResponse { id: u64, allow: bool },

    // === 会话 ===
    CreateSession { name: Option<CompactString> },
    SwitchSession { session_id: CompactString },
    DeleteSession { session_id: CompactString },
    RenameSession { session_id: CompactString, name: CompactString },

    // === 命令 ===
    RunCommand { command: CompactString },

    // === 配置 ===
    ReloadConfig,
    SetModel { model: CompactString },

    // === 生命周期 ===
    Quit,
}
```

### 4.3 生命周期

```
UserAction::SendMessage
  → CoreEngine.handle_action()
    → Agent.run() (streaming)
      → CoreEvent::StreamingDelta (多次)
      → CoreEvent::ToolCall { ... }
        → CoreEvent::PermissionNeeded { ... }
        ← UserAction::PermissionResponse { allow: true }
      → CoreEvent::ToolResult { ... }
    → CoreEvent::MessageComplete
```

---

## 5. Makepad GUI 设计

### 5.1 界面布局

```
┌──────────────┬───────────────────────────────────────────────┐
│              │  [Session Name]           [model] [tokens]    │
│   Sidebar    │  ──────────────────────────────────────────── │
│              │                                               │
│  ┌────────┐  │  ┌─────────────────────────────────────────┐ │
│  │ Sess 1 │  │  │ 👤 User                                 │ │
│  │ Sess 2 │  │  │ How do I refactor this module?          │ │
│  │ Sess 3 │  │  └─────────────────────────────────────────┘ │
│  │ Sess 4 │  │  ┌─────────────────────────────────────────┐ │
│  │        │  │  │ 🤖 Assistant                            │ │
│  └────────┘  │  │ Here's the plan...                      │ │
│              │  │                                          │ │
│  ┌────────┐  │  │ ```rust                                 │ │
│  │ + New  │  │  │ pub fn refactor() {                     │ │
│  └────────┘  │  │     // ...                              │ │
│              │  │  │ }                                     │ │
│              │  │  ```                                    │ │
│              │  └─────────────────────────────────────────┘ │
│              │  ┌─────────────────────────────────────────┐ │
│              │  │ 🔧 read ── src/main.rs                  │ │
│              │  │ 📄 content here...                      │ │
│              │  └─────────────────────────────────────────┘ │
│              │                                               │
│              │  ──────────────────────────────────────────── │
│              │  │ Type a message...              [Send]    │ │
│              │  ──────────────────────────────────────────── │
│              │  │ claude-sonnet │ 12.3K tokens │ ⚙ Config  │ │
└──────────────┴───────────────────────────────────────────────┘
```

### 5.2 组件树

```
MainView
├── Sidebar (220px)
│   ├── SessionList
│   │   ├── SessionItem (selected)
│   │   ├── SessionItem
│   │   └── ...
│   └── NewSessionButton
│
├── ChatArea (flex: 1)
│   ├── HeaderBar
│   │   ├── SessionTitle
│   │   ├── ModelSelector
│   │   └── TokenCounter
│   │
│   ├── MessageList (ScrollView)
│   │   ├── UserMessage
│   │   │   └── MarkdownContent
│   │   ├── AssistantMessage
│   │   │   └── MarkdownContent
│   │   │       ├── CodeBlock → syntax highlight
│   │   │       └── InlineCode
│   │   ├── ToolCallCard
│   │   │   ├── ToolIcon
│   │   │   ├── ToolName
│   │   │   └── ToolArgs (collapsed)
│   │   └── ToolResultCard
│   │       └── ResultContent (collapsed)
│   │
│   └── InputBar
│       ├── TextInput (multiline)
│       └── SendButton
│
└── StatusBar
    ├── ModelIndicator
    ├── TokenCounter
    └── ConfigButton
```

### 5.3 弹窗

| 弹窗 | 触发时机 | 内容 |
|------|---------|------|
| PermissionDialog | 工具需要权限 | 工具名、参数、Allow/Deny/Always 按钮 |
| CommandPalette | Ctrl+P 或 `/` | 搜索斜杠命令、会话切换 |
| ConfigDialog | 点击 ⚙ | 模型选择、provider 配置、主题 |

### 5.4 主题

暗色主题（默认，继承 zerostack 现有 style）：

| 属性 | 值 |
|------|-----|
| bg | `#1A1B26` |
| sidebar | `#1F2030` |
| user msg | `#2D2E3F` |
| asst msg | `#1A1B26` |
| code bg | `#0D0E1A` |
| accent | `#7C6FF0` |
| border | `#2D2E3F` |
| text | `#CDD6F4` |

亮色主题（后续添加）：

| 属性 | 值 |
|------|-----|
| bg | `#FFFFFF` |
| sidebar | `#F7F7F8` |
| user msg | `#F0F0F0` |
| asst msg | `#FFFFFF` |
| code bg | `#F5F5F5` |
| accent | `#6C5CE7` |
| border | `#E5E5E5` |
| text | `#1A1A2E` |

---

## 6. 线程与异步桥接

### 6.1 问题

Makepad 的事件循环运行在主线程，tokio 需要自己的 runtime。两者不能共享同一个线程。

### 6.2 桥接方案

```
┌───────────────────────────────────────────────────────┐
│                    主线程 (Makepad)                     │
│                                                        │
│   MakepadApp                                          │
│   ├── event_loop()                                    │
│   ├── 每帧轮询 CoreEvent (try_recv)                    │
│   │   CoreEvent::StreamingDelta ─▶ 更新 UI widget     │
│   │   CoreEvent::ToolCall ───────▶ 添加 ToolCard      │
│   │   CoreEvent::PermissionNeeded ▶ 弹出对话框        │
│   │                                                   │
│   └── UI 交互 ──────────────▶ UserAction              │
│       · 点击 Send ──────────▶ SendMessage             │
│       · 点击 Allow ─────────▶ PermissionResponse      │
│                                │                       │
│              ┌─────────────────┘                       │
│              │  tokio::mpsc::unbounded_channel         │
│              ▼                                         │
│   ┌──────────────────────────────────────┐            │
│   │         后台线程 (tokio)              │            │
│   │                                      │            │
│   │   CoreEngine::run()                  │            │
│   │   ├── 接收 UserAction                │            │
│   │   ├── 执行 agent / tool              │            │
│   │   └── 发送 CoreEvent                 │            │
│   │                                      │            │
│   └──────────────────────────────────────┘            │
└───────────────────────────────────────────────────────┘
```

### 6.3 GuiBridge 骨架

```rust
pub struct GuiBridge {
    action_tx: UnboundedSender<UserAction>,
    event_rx: UnboundedReceiver<CoreEvent>,
    _runtime_thread: thread::JoinHandle<()>,
}

impl GuiBridge {
    pub fn new(config: Config) -> Self {
        let (action_tx, action_rx) = unbounded_channel::<UserAction>();
        let (event_tx, event_rx) = unbounded_channel::<CoreEvent>();

        let runtime_thread = thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all().build().unwrap();
            rt.block_on(async move {
                let mut engine = CoreEngine::new(config);
                while let Some(action) = action_rx.recv().await {
                    let events = engine.handle_action(action).await;
                    for event in events { let _ = event_tx.send(event); }
                }
            });
        });

        Self { action_tx, event_rx, _runtime_thread: runtime_thread }
    }

    pub fn send_action(&self, action: UserAction) {
        let _ = self.action_tx.send(action);
    }

    pub fn poll_events(&mut self) -> Vec<CoreEvent> {
        let mut events = Vec::new();
        while let Ok(event) = self.event_rx.try_recv() {
            events.push(event);
        }
        events
    }
}
```

### 6.4 关键设计决策

| 决策 | 理由 |
|------|------|
| `unbounded_channel` | 避免阻塞，core 不会因 UI 忙而卡住 |
| `try_recv` 非阻塞轮询 | 每帧检查新事件，Makepad 不等待 |
| `current_thread` runtime | tokio 只需单线程，减少资源开销 |
| 事件批量处理 | 一帧内多个 StreamingDelta 攒在一起刷新 UI |
| 无锁设计 | 单向 channel，不需要 Mutex |

---

## 7. 执行阶段

### Phase 1: 核心引擎提取 (`crates/core`)

| Step | 内容 | 验证 |
|------|------|------|
| 1.1 | 创建 `crates/core` 骨架，定义 `CoreEvent` + `UserAction` | 编译通过 |
| 1.2 | 提取 `config`, `session`, `permission` 模块 | 现有测试通过 |
| 1.3 | 提取 `agent` (builder, runner, tools) | 现有测试通过 |
| 1.4 | 提取 `provider`, `extras` (mcp, subagents, memory, loop) | 现有测试通过 |
| 1.5 | 实现 `CoreEngine` 主结构体 | 单元测试通过 |
| 1.6 | 让 `src/main.rs` 通过 core crate 运行，验证 TUI 不受影响 | 手动测试 |

**退出标准**: `cargo test` 全部通过，现有 TUI 正常使用。

### Phase 2: Makepad GUI (`crates/gui`)

| Step | 内容 | 验证 |
|------|------|------|
| 2.1 | 搭建 Makepad 项目骨架，接入 `GuiBridge` | 窗口可启动 |
| 2.2 | 实现 `MainView` 布局（Sidebar + ChatArea + InputBar） | 布局正确渲染 |
| 2.3 | 实现消息渲染（Markdown + 代码高亮） | 消息正确显示 |
| 2.4 | 实现工具调用卡片（ToolCallCard + ToolResultCard） | 工具调用可视化 |
| 2.5 | 实现权限弹窗（PermissionDialog） | 权限流程可用 |
| 2.6 | 实现会话管理（侧边栏 CRUD + 切换） | 会话操作正常 |
| 2.7 | 实现斜杠命令面板（CommandPalette） | 命令可用 |
| 2.8 | 实现状态栏 + 主题切换 | 状态信息正确 |
| 2.9 | CLI 集成（`zerostack --gui` 启动 GUI） | CLI 参数生效 |

**退出标准**: `zerostack --gui` 启动完整 GUI，功能与 TUI 对等。

### Phase 3: 完善与测试

| Step | 内容 |
|------|------|
| 3.1 | 端到端测试 |
| 3.2 | 性能优化 |
| 3.3 | 文档更新 |

**退出标准**: CI 覆盖 GUI 构建，README 文档更新。

### 依赖关系

```
Phase 1 ──────────▶ Phase 2 ──────────▶ Phase 3
(核心提取)          (GUI 开发)          (完善)
    │
    └── 现有 TUI 始终可用，不受影响
```

---

## 8. 风险与缓解

| 风险 | 缓解 |
|------|------|
| Makepad 与 tokio 事件循环冲突 | 后台线程 + channel 桥接，已验证可行 |
| Makepad API 稳定性 | 固定 Makepad 版本，关注上游更新 |
| 核心提取破坏现有 TUI | Phase 1 每步都运行 `cargo test`，渐进式迁移 |
| Markdown 渲染复杂度 | Makepad 有内置文本渲染，代码高亮可用 syntect |
| 性能（大量 streaming 事件） | 批量处理 + 帧率限制（60fps） |

---

## 9. 未决问题

- Makepad 的文本输入组件是否支持多行输入和 IME（中文输入）？需要在实际开发中验证。
- 代码高亮方案：syntect 还是 tree-sitter？建议先用 syntect（纯 Rust，无编译依赖）。
- 会话数据存储是否需要与 TUI 共享？建议共享，使用相同的 JSON 文件格式。