---
name: complete-c-run-script
overview: 在 `<cmd> <TAB>` 形态触发时，从共享 registry 查注册脚本并 spawn 执行，把 stdout 首行作为单一候选补全到光标处并加尾空格；未注册回退现有文件名补全分支；其他异常一律静默 no-op。
todos:
  - id: share-registry-via-rc
    content: 把 main.rs 的 completions 升级为 Rc<RefCell<HashMap>>，dispatch 调用点用 borrow_mut 桥接到 run_complete 现有签名
    status: completed
  - id: helper-holds-registry
    content: ShellHelper 新增 completions 字段并改 new 为带参签名，main.rs 在 set_helper 时透传 Rc::clone
    status: completed
    dependencies:
      - share-registry-via-rc
  - id: impl-command-level-completion
    content: 在 Completer::complete 入口新增「严格 cmd+空白 → 查 registry → spawn 脚本取首行」分支，未命中走既有 complete_filename_arg，脚本异常静默 no-op
    status: completed
    dependencies:
      - helper-holds-registry
  - id: add-helpers-and-unit-tests
    content: 新增 extract_command_only / run_completer_script 私有 helper 与 extract_command_only 的边界单元测试
    status: completed
    dependencies:
      - impl-command-level-completion
  - id: verify-end-to-end
    content: cargo build + cargo test；手写 /tmp/completer.sh 集成验证：注册后 docker<TAB> 补全为 "docker run "；脚本失败/未注册/已有参数前缀场景无回归
    status: completed
    dependencies:
      - add-helpers-and-unit-tests
---

## Product Overview

为 codecrafters Rust shell 接入 `complete -C` 注册的补全脚本：当用户在 REPL 输入 `<cmd>` + 空白 + TAB 时，shell 调起对应已注册脚本（外部子进程），读取 stdout 首行作为单一补全候选直接替换光标处空 token，并补一个尾空格。未注册命令或脚本异常时无感回退到当前文件名补全逻辑。

## Core Features

- 严格触发：仅 `<cmd> <空白...> <TAB>`（首词后已进空白、尚未键入下一 token）形态触发脚本调用
- registry 命中：spawn 脚本（独立进程）→ 等待结束 → 读 stdout → 取首行 trim 末尾换行作为候选 → line 变成 `<cmd> <候选> `（尾空格，光标停末尾）
- registry 未命中：完全沿用现有 `complete_filename_arg`（cwd 文件名 / 双 TAB / 目录 `/`）
- 脚本异常静默 no-op：spawn 失败 / 非零退出 / stdout 空 / 多行（取首行容差）/ 子进程错误，一律 line 不变、不响铃、不报错
- 上 stage `complete -C` 注册行为不回归（dispatch 路径与 `-p` 查询均通过共享 registry）

## Tech Stack

- 沿用现有项目：Rust + 标准库 + 已存在的 rustyline；无新增 crate、不改 Cargo.toml
- 跨 helper / dispatch 共享：`std::rc::Rc<std::cell::RefCell<HashMap<String, String>>>`（单线程 REPL，无锁开销）
- 子进程：`std::process::Command::output()`（自动 wait，stdout 一次性收齐，避免「读到部分输出」的题目陷阱）

## Implementation Approach

### 核心思路

1. 把 `main.rs` 中已有的 `let mut completions: HashMap<...>` 升级为 `let completions = Rc::new(RefCell::new(HashMap::new()));`，并把同一 `Rc` 同时交给：

- `ShellHelper::new(completions.clone())` —— 让 TAB 路径读注册表
- dispatch 中 `run_complete` 调用点 —— 写注册表（沿用现有 `&mut HashMap` 签名，用 `&mut *completions.borrow_mut()` 桥接，零签名扩散）

2. `completion.rs::ShellHelper` 加 registry 字段（同类型 `Rc<RefCell<...>>`）；改 `new()` 为带参签名。
3. `Completer::complete` 入口已有「`line[..pos]` 含空白 → `complete_filename_arg`」分支。**在进入 `complete_filename_arg` 之前**插入「严格命令级补全」判定：

- 用新 helper `extract_command_only(line_to_pos) -> Option<&str>` 严格识别「光标左侧恰好等于：可选前导空白 + 单个命令 token + ≥1 个空白结尾」
- 命中后查 `registry.borrow().get(cmd)`：未命中 → 落回现有 `complete_filename_arg`；命中 → 调 `run_completer_script(path)` → `Ok(t)` 返回 `(pos, vec![Pair{display:t, replacement: format!("{} ", t)}])`；任何 `None` / 失败 → 落回现有 `complete_filename_arg`（按用户决策"未命中=回退现有"，且失败不响铃，自然与回退路径合流）。

4. **关键修正**：用户决策 3 是"脚本失败=静默 no-op"，与决策 2"未命中=回退文件名补全"语义不同；为了避免回归（例如 `docker <TAB>` 注册了但脚本坏掉时去扫 cwd 列出文件污染体验），脚本失败一律 `Ok((pos, Vec::new()))` 静默而**不**回退到文件名补全；只有「registry 未命中」走 fallback。这样精确符合两条决策。
5. 子进程契约：`Command::new(path).output()` 自带 wait + 收齐 stdout，避免部分读取；非零退出码视为失败（即便 stdout 有内容也丢弃，与"严格遵循题目"决策一致）；stdout bytes → `String::from_utf8` 失败按 no-op；按 `\n` split 取首个非空行（容忍 CRLF：尾部 `\r` 一并 trim）。

### 关键决策与权衡

- **触发位置选 `Completer::complete` 入口而非 `complete_filename_arg` 内部**：保留 `complete_filename_arg` 单一职责（文件名补全），新功能独立成「命令级补全分支」，零侵入既有状态机；同时方便后续 stage 在同一切点扩展「部分前缀 → 传 COMP_LINE」而不与文件名路径耦合。
- **注册表用 `Rc<RefCell<...>>` 而非全局 static**：与现有 main 局部所有权风格一致，零模块越界；rustyline `Editor` 拿走 helper 所有权后，main 仍持一份 `Rc` 用于 dispatch 写入，互不阻塞；`RefCell` 借用规则在单线程 + 不嵌套调用下天然安全（TAB 与 dispatch 走 REPL 串行节奏，不会并发借用）。
- **`Command::output()` 而非 `spawn + read`**：题目明确"wait for the completer to finish before inserting"，`output()` 是教科书一行答案；避免管道缓冲/部分读取/僵尸进程三类隐患。
- **多行容差取首行**：题目保证 exactly one line，但脚本以 `print` / `echo` 收尾必带 `\n`，所以 `lines().next()` 是稳态实现；超出首行的内容静默丢弃，与"严格遵循题目"决策一致。

### 性能

- TAB 是低频交互；spawn 一次外部进程的开销远高于任何 hashmap 查找，复杂度无优化空间。registry 读侧 `borrow()` O(1) 哈希；写侧仅在 `complete -C` 命令时发生，与执行频次匹配。
- 启动期 `path_executables` 缓存策略不动。

## Implementation Notes

- **借用边界**：在 `Completer::complete` 内**先**克隆出 `path: Option<String> = registry.borrow().get(cmd).cloned()` 后立刻 drop 借用，再调 `Command::output()`（避免 spawn 期间 RefCell 借用挂着，虽然单线程也不会冲突，但缩短借用窗口是好习惯）。
- **dispatch 桥接**：保留 `run_complete(... , registry: &mut HashMap<...>)` 现有签名；调用点改为 `run_complete(&mut *sink, &mut *err_sink, args, &mut *completions.borrow_mut())`。`borrow_mut()` 临时持有可变借用直至语句结束，与 helper 的读借用串行不冲突。
- **状态机不污染**：命令级补全分支返回前清空 `last_tab_prefix` / `last_tab_arg_key`（与现有 `complete_filename_arg` 进入时的清空时机对齐），避免「先命令名 BEL 再 docker <TAB>」之类的串味。
- **stdout 解析**：`output.stdout` 是 `Vec<u8>`；按 `\n` 切首段，再 trim 尾随 `\r`；空字符串 → no-op；候选含中间空白时不做拆分（视为单个候选整体）。
- **不向脚本传 stdin / argv / env 上下文**：本 stage 显式禁止；`Command::new(path)` 零参，继承父进程 env（与 bash 真实行为一致，也便于脚本写 `#!/usr/bin/env python3`）。
- **blast radius**：仅改 `main.rs`（共享句柄）、`completion.rs`（入口分支 + 新 helper + 测试）；不动 builtins.rs 的 `run_complete` 逻辑、parser、exec、redirect。
- **日志**：补全是交互路径，所有异常一律静默；绝不向 stdout/stderr 喷错误（会污染用户输入区与 tester 输出）。

## Architecture Design

```mermaid
flowchart LR
    A[REPL readline TAB] --> B{Completer::complete}
    B -->|line[..pos] 无空白| C[命令名补全 既有路径]
    B -->|含空白| D{extract_command_only<br>命中?}
    D -->|否, 已进入参数区| E[complete_filename_arg<br>既有路径]
    D -->|是, &lt;cmd&gt;+空白+TAB| F{registry.borrow get cmd}
    F -->|未命中| E
    F -->|命中 path| G[Command::new path .output]
    G -->|失败 / 空 / 非零| H[no-op: pos, vec!]
    G -->|首行 t| I[Pair{display:t,<br>replacement:t+空格}]

    subgraph main.rs loop
      M1[Rc&lt;RefCell&lt;HashMap&gt;&gt;]
      M2[dispatch 'complete'<br>run_complete借borrow_mut]
    end
    M1 -.share Rc.-> B
    M1 -.share Rc.-> M2
```

## Directory Structure

```
codecrafters-shell-rust/
└── src/
    ├── main.rs        # [MODIFY]
    │                  # - 顶部 use 追加：std::rc::Rc, std::cell::RefCell
    │                  # - 旧 `let mut completions: HashMap<...>` → 
    │                  #   `let completions: Rc<RefCell<HashMap<String,String>>> = Rc::new(RefCell::new(HashMap::new()));`
    │                  # - `editor.set_helper(Some(ShellHelper::new()));` 改为
    │                  #   `editor.set_helper(Some(ShellHelper::new(completions.clone())));`
    │                  #   (注意：必须在 set_helper 之前 / 之后都 OK，但 completions 声明需移到 set_helper 之前)
    │                  # - dispatch 中 "complete" 分支调用改为
    │                  #   `run_complete(&mut *sink, &mut *err_sink, args, &mut *completions.borrow_mut())`
    │                  # - 其他逻辑零改动
    └── completion.rs  # [MODIFY]
                       # - use 追加：std::cell::RefCell, std::collections::HashMap,
                       #   std::process::Command, std::rc::Rc
                       # - struct ShellHelper 新增字段
                       #   `completions: Rc<RefCell<HashMap<String, String>>>`
                       # - impl ShellHelper::new 签名改为
                       #   `pub fn new(completions: Rc<RefCell<HashMap<String, String>>>) -> Self`
                       #   字段透传，其他字段保持
                       # - Completer::complete 入口分支调整：
                       #     首词逻辑保持；进入「含空白」分支时先调用
                       #     `extract_command_only(&line[..pos])`，命中且 registry 命中则
                       #     调 `run_completer_script(&path)`，成功取首行 → 直接返回
                       #     `(pos, vec![Pair{display: t.clone(), replacement: format!("{} ", t)}])`，
                       #     并清空 last_tab_prefix / last_tab_arg_key；
                       #     失败 / 脚本异常 → 返回 (pos, vec![])（不回退到文件名补全）；
                       #     未命中或非严格形态 → 沿用既有 self.complete_filename_arg(line, pos)
                       # - 新增模块私有 helper：
                       #   * fn extract_command_only(line_to_pos: &str) -> Option<&str>
                       #     语义：严格识别「可选前导空白 + 单 token + ≥1 空白结尾」；
                       #     返回 Some(cmd)；其他形态（无空白尾 / 多 token / 引号 / 空字符串）→ None；
                       #     实现：rev 找首个非空白位置 idx，若 idx == len 说明全空白(此处不命中
                       #     因 helper 由 complete() 入口的「含空白」分支调用，cmd 区不可能全空)；
                       #     在 idx 之前找首个空白边界，确保命令 token 不含空白；命令 token 内不含 / ' " \\ 等
                       #     歧义字符的硬约束可放宽（tester 不构造此类 cmd 名），保持实现简洁
                       #   * fn run_completer_script(path: &str) -> Option<String>
                       #     语义：spawn `Command::new(path).output()`；
                       #     - Err / status 非 success → None
                       #     - stdout 不是合法 UTF-8 → None
                       #     - 取首行（split_once('\n') 或 lines().next()），trim 尾随 '\r'
                       #     - 空字符串 → None
                       #     - 否则 → Some(line)
                       # - tests 模块追加单元测试：
                       #   * extract_command_only_basic / multispace / not_after_arg / empty / leading_ws
                       #   * （run_completer_script 走集成手测，不写依赖外部脚本的 cargo test）
```

## Key Code Structures

```rust
// completion.rs
pub struct ShellHelper {
    path_executables: Vec<String>,
    last_tab_prefix: std::cell::Cell<Option<String>>,
    last_tab_arg_key: std::cell::Cell<Option<(String, String)>>,
    completions: std::rc::Rc<std::cell::RefCell<std::collections::HashMap<String, String>>>,
}

impl ShellHelper {
    pub fn new(
        completions: std::rc::Rc<std::cell::RefCell<std::collections::HashMap<String, String>>>,
    ) -> Self;
}

/// 严格识别 `<cmd>` 后接 ≥1 空白的形态；其他形态返回 None
fn extract_command_only(line_to_pos: &str) -> Option<&str>;

/// 运行已注册补全脚本，返回首行候选；任何异常返回 None
fn run_completer_script(path: &str) -> Option<String>;
```