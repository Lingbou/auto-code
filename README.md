# AutoCode

PRD 驱动的无监督循环迭代进化开发工具。

在目标项目根目录放 `PRD.md`，然后执行：

```bash
./autocode claude
```

或：

```bash
./autocode opencode
```

它会进入持续迭代：
- 读取 PRD
- 调用对应 Provider 产生命令
- 执行命令并验证
- 记录日志和 checkpoint
- 继续下一轮

停止条件只有一个：`max-runtime` 到达。
到达后会在当前轮完成后再停止。

## 使用方式

1. 构建二进制
```bash
cargo build --release
```

2. 放到目标项目目录
```bash
cp target/release/autocode /path/to/project/autocode
chmod +x /path/to/project/autocode
```

3. 在目标项目目录准备 `PRD.md`
- 按“项目上下文 / 需求列表 / 验收标准”结构直接编写

4. 运行
```bash
cd /path/to/project
./autocode claude
```

说明：
- `claude` 使用非交互模式（`claude -p --output-format json`），会真正返回可解析文本。
- 运行目录建议是独立项目目录，不要放在另一个 Rust 项目的子目录里。

## 命令参数

```bash
./autocode <claude|opencode> [--max-runtime 4h] [--provider-timeout 20m] [--dry-run] [--verbose]
```

- `claude`: 使用 `claude` 命令
- `opencode`: 使用 `opencode` 命令
- `--max-runtime`: 唯一收敛条件，默认 `4h`
- `--provider-timeout`: 单次 AI 响应超时，默认 `20m`

## 运行产物

运行文件都写在项目目录 `.autocode/` 下：
- `.autocode/logs/events.log`
- `.autocode/logs/ai_output.log`
- `.autocode/logs/terminal_output.log`
- `.autocode/checkpoints/run_YYYYMMDD_HHMMSS/`

## PRD 必填结构

`PRD.md` 至少要有：
- `## 项目上下文`
- `## 需求列表`
- `## 验收标准`

当前版本不做命令安全拦截。

## 常见问题

1. 看起来在跑，但没有执行 AI 命令
- 查看 `.autocode/logs/events.log`，若出现 `AI_NO_COMMANDS`，通常是 Provider 返回了错误文本（比如未登录）。
- 如果日志里有 `Not logged in · Please run /login`，先在项目目录手动执行一次 `claude` 并登录。
- 如果日志里是 `provider timed out after ...`，继续加大 `--provider-timeout`，例如 `--provider-timeout 30m`。

2. `cargo test` 通过了，但项目里没代码
- 新版本会要求当前目录必须有本地 `Cargo.toml` 才运行 `cargo test/build/clippy`。
- 如果没有，会在日志中提示先 `cargo init`，避免误跑到父目录 Rust 项目。
