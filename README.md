# AutoCode

Rust 实现的终端 AI 编码 CLI，目标体验对齐 Claude Code / OpenCode。  
当前版本已支持：

1. 默认全屏 TUI 交互模式（`autocode`，TTY 环境）
2. provider 选择（`claude` / `opencode` / `auto`）
3. 插件化 PRD runner（保留并复用旧的 PRD 自动循环引擎）

## 快速开始

构建：

```bash
cargo build --release
```

交互模式：

```bash
./target/release/autocode
```

说明：
1. 在 TTY 里默认进入全屏 TUI
2. 在非 TTY（如管道输入）会自动降级到行式交互模式

PRD 自动循环（插件）：

```bash
cd /path/to/project
./autocode plugin prd-runner run --provider claude --max-runtime 10m
```

等价别名：

```bash
./autocode prd run --provider claude --max-runtime 10m
```

兼容旧入口：

```bash
./autocode claude --max-runtime 10m
./autocode opencode --max-runtime 10m
```

## 命令概览

```bash
autocode                        # 交互模式
autocode run ...                # 兼容入口，转发到 prd-runner run
autocode plugin list            # 插件列表
autocode plugin prd-runner ...  # 插件标准入口
autocode prd ...                # prd-runner 别名入口
autocode doctor                 # 诊断 provider 与环境
```

常用 PRD 命令：

```bash
autocode prd init
autocode prd validate
autocode prd run --provider auto --max-runtime 10m
autocode prd resume --run-id run_YYYYMMDD_HHMMSS
autocode prd status [--run-id ...]
```

TUI 内置命令：

```bash
/help
/provider auto|claude|opencode
/plugin <id> <cmd> [args...]
/prd <cmd> [args...]
/clear
/exit
```

## PRD 文件要求

项目根目录需存在 `PRD.md`，至少包含：

1. `## 项目上下文`
2. `## 需求列表`
3. `## 验收标准`

## 输出与产物

运行产物位于项目目录 `.autocode/`：

1. `.autocode/logs/events.log`
2. `.autocode/logs/ai_output.log`
3. `.autocode/logs/terminal_output.log`
4. `.autocode/checkpoints/run_YYYYMMDD_HHMMSS/`

## 常见问题

1. Provider 未登录
- 日志出现 `Not logged in` / `Please run /login` 时，先执行 provider 自身登录（例如 `claude /login`）。

2. 没有执行到 AI 命令
- 检查 `events.log` 是否出现 `AI_NO_COMMANDS`。
- 查看 `ai_output.log` 判断是模型无命令输出还是格式不符合。

3. provider 不可用
- 运行 `autocode doctor` 检查 `claude` / `opencode` 是否在 `PATH` 中可执行。
