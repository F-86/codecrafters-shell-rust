---
name: stderr-redirection-2-operator
overview: 为 shell 增加 `2>` stderr 重定向支持：扩展 tokenizer/parser 识别 `2>`，引入可重定向的 err_sink，将所有 builtin 错误信息、未找到命令信息、`type` not found 全部改走 err_sink，外部命令补 `Stdio::from(File)` 重定向子进程 stderr。
todos:
  - id: extend-tokenize-2gt
    content: 在 src/parser.rs 的 State::Normal `>` 分支并入 `current == "2"` 升级为 `"2>"` 的逻辑，新增 ≥3 个 tokenize 层单测覆盖空格/紧贴/引号内场景
    status: completed
  - id: extend-parsed-command-stderr
    content: 为 ParsedCommand 新增 stderr_redirect 字段，扩展 parse 循环识别 `"2>"` 并填充该字段，新增 ≥4 个 parse 层单测（含与 `>` 共存、`2>` 缺目标报错）
    status: completed
    dependencies:
      - extend-tokenize-2gt
  - id: refactor-builtins-err-sink
    content: 在 src/main.rs 抽出 open_err_sink，将 run_pwd / run_type 签名追加 err_sink 参数，把错误分支改为 writeln!(err_sink, ...)
    status: completed
    dependencies:
      - extend-parsed-command-stderr
  - id: wire-repl-stderr-redirect
    content: REPL 主循环准备 err_sink；cd 错误、command not found、外部命令 spawn 错误改走 err_sink；外部命令分支补 .stderr(Stdio::from(File)) 物化
    status: completed
    dependencies:
      - refactor-builtins-err-sink
  - id: cargo-test-and-build
    content: 运行 cargo test 验证既有 45 + 新增 ≥7 个测试全绿，cargo build --release 无警告
    status: completed
    dependencies:
      - wire-repl-stderr-redirect
  - id: e2e-verify-spec-samples
    content: 在 /tmp/quz 与 /tmp/bar 端到端回放 spec 三条样例（ls 错误、echo 空文件、cat 部分错误），验证文件内容与终端表现后清理
    status: completed
    dependencies:
      - cargo-test-and-build
---

## 产品概述

为 Rust 实现的 shell 增加 `2>` 操作符支持，将命令的标准错误输出重定向到指定文件；与 stdout 重定向架构对称，复用既有 sink 抽象与外部命令 Stdio 物化路径。

## 核心功能

- **`2>` 操作符识别**：tokenize 层把 `2>` 作为独立 token 切出（紧贴形态如 `cmd 2>file` 与空格形态如 `cmd 2> file` 均生效）；引号内 / 反斜杠转义后的 `2>` 仍按字面量。
- **结构化解析**：`ParsedCommand` 新增 `stderr_redirect: Option<String>` 字段；`parse` 单次扫描同时识别 `>` / `1>` / `2>`，目标文件缺失复用 `MissingRedirectTarget` 错误。
- **stderr 重定向语义**：
- 文件不存在则创建、存在则截断（与 stdout 处理对称）。
- 即使命令未产生任何 stderr 输出，目标文件也被预先创建为空（如 `echo Maria 2> err.txt`）。
- stdout 路径不受影响，仍输出到终端（或 `>` 重定向的目标）。
- **全量 builtin 错误改走 err_sink**：`cd: ...: No such file or directory`、`pwd` 的 cwd 错误、`type xxx: not found`、未找到命令的 `xxx: command not found`、外部命令 spawn 失败等错误信息统一通过 `err_sink: &mut dyn Write` 输出，使 `2>` 可捕获。
- **外部命令 stderr 物化**：`Command::stderr(Stdio::from(File))`；无 `2>` 时保持 `inherit` 默认行为。
- **stdout / stderr 同时存在**：`> ` 与 `2>` 可在同一行任意顺序共存（如 `cmd > out 2> err`、`cmd 2> err > out`），互不干扰。

## 验证场景（spec 三条端到端样例）

1. `ls nonexistent 2> /tmp/quz/baz.md` → 文件含 `ls: nonexistent: No such file or directory`，终端无 stderr。
2. `echo Maria file cannot be found 2> /tmp/quz/foo.md` → stdout 在终端，目标文件被创建为空。
3. `cat /tmp/bar/pear nonexistent 2> /tmp/quz/quz.md` → stdout（`pear` 内容）在终端，目标文件含 cat 的错误信息。

## 技术栈

- 语言/工具链：Rust（沿用现有 crate，零新增依赖）
- 标准库：`std::fs::File`、`std::io::{self, Write}`、`std::process::{Command, Stdio}`
- 测试：`cargo test` 内建 unit test 模块；端到端用 `./target/release/codecrafters-shell` 回放 spec

## 实现策略

- **对称扩展，不引入新模式**：完全复用上一 stage 已建立的「tokenize 切操作符 → parse 归一化字段 → REPL 准备 sink → builtin 写 sink / 外部命令物化 Stdio」四段式架构，把 `2>` 平行加入。**禁止**任何 unsafe / libc dup2 / fork 自管理路径。
- **tokenize 复用 `1>` 合并模式**：Normal 态遇到 `>` 时新增「`in_token && current == "2"` → 升级为 `"2>"`」分支，与既有 `current == "1"` 分支并列，保持引号内 / 转义后字面量语义不变。既有 45 个测试中无任何用例含 `2>`，零回归风险（已搜索确认）。
- **parse 单次扫描多字段填充**：`parse()` 循环中 `match tok.as_str()` 同时处理 `>` / `1>` / `2>`，目标 token 缺失统一返回 `MissingRedirectTarget`；重复 `2>` 取最后一次（与 bash / 上一 stage 重复 `>` 处理一致）。
- **err_sink 全量贯穿（q1=A，q2=B）**：新增 `open_err_sink(stderr_redirect: Option<&str>) -> io::Result<Box<dyn Write>>`，无重定向时返回 `Box::new(io::stderr())`，有则 `File::create`（q3=A：无条件截断创建）。`run_pwd`、`run_type` 函数签名追加 `err_sink: &mut dyn Write` 参数，所有错误分支改为 `writeln!(err_sink, ...)`；REPL 中 `cd` 错误、`command not found`、外部命令 spawn 失败也走 `err_sink`；**parse 错误本身**与 **sink 写入失败**仍保留 `eprintln!`（属 shell 自身错误，无命令上下文，bash 同样不可被命令的 `2>` 捕获）。
- **外部命令分支对称扩展**：在已有 `stdout` 物化代码块后追加 `stderr` 物化（`File::create(path)` → `Stdio::from(file)` → `.stderr(stdio)`），无 `2>` 时仍 `Stdio::inherit()`。
- **预先创建空文件（q3=A）**：`open_err_sink` 在「无错误信息」场景下也已调用 `File::create`；外部命令路径同样在物化阶段 `File::create`，即便子进程未写 stderr，文件也已存在为空。

## 性能与复杂度

- tokenize 仍是 O(n) 单次扫描；parse 在原有线性遍历上仅多一次字符串比较，复杂度不变。
- 文件 fd：每条命令最多额外 1 个文件 fd（err_sink）；命令结束后 `Box<dyn Write>` 析构自动 close，无泄漏。
- REPL 单线程，所有 I/O 同步，无并发问题。

## 实现注意

- **不要**让 `run_echo` 接收 err_sink——它无错误输出（写入失败仍由顶层 `?` 报到 `eprintln!`）；只 `run_pwd` / `run_type` 需要。
- **不要**改 `eprintln!("{}", e)`（parse error）与 `eprintln!("shell: write error: ...")`，它们是 shell 自身的诊断（bash 中也不可被命令上下文的 `2>` 捕获），保持原状降低回归面。
- **不要**改 `ParseError` 枚举或 `Display` 文案——直接复用 `MissingRedirectTarget` 即可。
- **不要**在 `run_pwd`/`run_type` 内 `flush` err_sink——`Box<dyn Write>` 析构（drop）时由 `BufWriter` 行为决定；对 `File` 与 `io::Stderr` 均无显式 flush 需求。
- 外部命令路径：在 spawn 之前 `drop(sink)` 已有；本 stage 新增 `err_stdio` 物化时**不需要**额外 drop err_sink（err_sink 与外部命令 stderr fd 独立，可并存）。但为对称简洁，建议在外部命令分支统一在 spawn 前 `drop(err_sink)`，避免父进程持有半空文件句柄。

## 架构图（仅展示数据流）

```mermaid
flowchart LR
  A[stdin line] --> B[parser::tokenize<br/>2> 独立 token]
  B --> C[parser::parse<br/>ParsedCommand{argv, stdout_redirect, stderr_redirect}]
  C --> D{builtin?}
  D -->|yes| E[open_sink + open_err_sink<br/>Box<dyn Write>]
  E --> F[run_echo / run_pwd / run_type<br/>writeln! sink/err_sink]
  D -->|no| G[Command::new<br/>.stdout(File or inherit)<br/>.stderr(File or inherit)]
```

## 目录结构

```
codecrafters-shell-rust/
├── src/
│   ├── parser.rs   # [MODIFY] tokenize Normal 态新增 current=="2" 升级合并为 "2>"；ParsedCommand 增 stderr_redirect 字段；parse 循环识别 "2>" 填充该字段；测试模块新增 tokenize 层 3 个 + parse 层 4 个共 ≥7 个用例
│   └── main.rs     # [MODIFY] 新增 open_err_sink；run_pwd/run_type 签名追加 err_sink 参数并改写错误分支；REPL 主循环准备 err_sink、cd 错误/command not found/外部命令 spawn 失败改走 err_sink；外部命令分支补 .stderr(Stdio::from(File)) 物化
└── （Cargo.toml / Cargo.lock / your_program.sh 不动）
```

## 关键接口（仅 1 处变更，便于精确实施）

```rust
// parser.rs
pub struct ParsedCommand {
    pub argv: Vec<String>,
    pub stdout_redirect: Option<String>,
    pub stderr_redirect: Option<String>,  // 本 stage 新增
}

// main.rs
fn open_err_sink(stderr_redirect: Option<&str>) -> io::Result<Box<dyn Write>>;
fn run_pwd(sink: &mut dyn Write, err_sink: &mut dyn Write) -> io::Result<()>;
fn run_type(sink: &mut dyn Write, err_sink: &mut dyn Write, args: &[String]) -> io::Result<()>;
```

## Agent Extensions

本任务为单仓 Rust 小项目的对称扩展，所有修改集中于 2 个文件，无需调用 MCP / Skill / SubAgent。不输出额外扩展。