---
name: register-declare-builtin
overview: 在 src/builtins.rs 的 BUILTINS 数组追加 "declare"，并在 main.rs dispatch 中加一个空 arm 占位，使 `type declare` 输出 `declare is a shell builtin`，本阶段不实现 declare 行为。
todos:
  - id: register-declare-builtin
    content: 在 src/builtins.rs 的 BUILTINS 数组末尾追加 "declare"，同时在 src/main.rs 的 match cmd 表中 _ => run_external 兜底之前插入 "declare" => {} 占位 arm 并附本阶段占位注释
    status: completed
  - id: verify-build-and-behavior
    content: 运行 cargo build 确认无新 warning，启动 ./your_program.sh 验证 type declare 输出 "declare is a shell builtin"、直接输入 declare foo=bar 无 not found 报错、既有内建行为零回归
    status: completed
    dependencies:
      - register-declare-builtin
---

## 产品概述

为 Rust 实现的 shell 注册 `declare` 内建命令。本阶段仅完成"注册"动作，使 `type declare` 能返回 `declare is a shell builtin`，**不实现** `declare variable=value` / `declare -p variable` 等子命令行为（后续阶段再实现）。

## 核心功能

- **type 查询命中**：在 REPL 输入 `type declare`，输出 `declare is a shell builtin`（走 stdout，可被 `1>` / `>` 重定向）。
- **dispatch 占位**：在 REPL 输入 `declare ...` 不再走 `run_external` 报 `command not found`；本阶段无任何输出，与 bash 中"已注册但 noop"的占位语义一致，为后续阶段挂载真实实现预留入口。
- **TAB 补全联动**：`dec<TAB>` 自动补全为 `declare`（无需额外改动，BUILTINS 单一事实来源天然驱动）。
- **既有行为零回归**：`echo` / `exit` / `type` / `pwd` / `cd` / `complete` / `jobs` / `history` 全部既有内建行为保持不变。

## 技术栈选型

沿用现有项目栈，**不引入任何新依赖**：

- Rust edition 2024 / rustc ≥ 1.95
- 既有依赖：`anyhow` / `thiserror` / `bytes` / `rustyline` 均无需触碰

## 实施策略

### 高层方案

本阶段是典型的"在单一事实来源数组追加 + dispatch 占位 arm"两点最小改动，复刻项目此前 `cd` / `complete` / `jobs` / `history` 等内建首次注册时的同款做法：

1. **改动点 A — `src/builtins.rs` 第 18-20 行的 `BUILTINS` 常量数组**：在末尾追加 `"declare"`。

- 该数组是 `run_type`（第 173 行 `BUILTINS.contains(&target)`）和 TAB 补全（`completion.rs` 第 402 行 `for name in BUILTINS`）的**唯一**数据源；追加一项即同时打通 `type` 命中和 TAB 补全两条路径。
- 数组上方注释 `// 后续阶段新增内建（如 pwd/cd）时只需在此处追加` 已为本次追加预留语义，无需修改注释本体。

2. **改动点 B — `src/main.rs` 第 272-496 行 `match cmd` 表中加 `"declare" => {}` 占位 arm**：

- 必须放在 `_ => run_external(...)` 之前，避免被兜底分支吞掉而走外部命令路径报 `declare: command not found`。
- arm 体为空块 `{}`，**不调用任何子函数、不写 stdout / stderr**，与题面 Notes "only need to register" 严格一致。
- 加 2-3 行 `//` 注释说明"本阶段占位、行为留待后续阶段；后续阶段把 `{}` 替换为 `run_declare(...)` 调用即可"，对齐既有 arm 的注释密度。

### 关键技术决策与权衡

- **为何加 dispatch 占位而非只改 BUILTINS**：仅改 BUILTINS 数组确实能让 tester 通过（tester 只跑 `type declare`），但用户在 REPL 直接输入 `declare foo=bar` 时会落入 `_ => run_external(...)` 分支并打印 `declare: command not found`——这与 bash 行为不符，也违反「`type` 声称是 builtin」与「执行时按外部命令处理」的契约一致性。占位 arm 是零成本（编译后即一个跳转表项），换来契约自洽与后续阶段零额外改动接入点，符合 KISS + YAGNI 的合理平衡。
- **为何不预先创建 `run_declare` 函数骨架**：题面 Notes 明确「only need to register」；预先写空函数会引入未使用的 `pub fn` 触发 dead_code 警告，或被迫加 `#[allow(dead_code)]` 引入语义噪音。空 arm 是最干净的占位形式。
- **为何不动 `completion.rs` / `exec.rs`**：补全候选源直接遍历 `BUILTINS`（已验证 `completion.rs` 第 67、402 行），追加自动生效；`run_external` 仅通过 `find_in_path` 与 PATH 耦合，与 BUILTINS 数组无依赖关系。

### 执行注记

- **Blast radius 严格控制**：仅两个文件、合计新增 ≤ 5 行（数组追加 1 行 + dispatch arm 含注释 3-4 行），无任何既有行删除或重排，无 API 变更，零 borrow 影响。
- **零性能影响**：`BUILTINS.contains(&target)` 是线性扫描 9 个静态字符串，多 1 项 O(1) 量级无感；dispatch 多 1 个 match arm 编译器会优化为跳转表项，无运行时开销。
- **日志/输出契约**：本阶段不新增任何 stdout / stderr 字节，保持与 tester 输出契约 100% 一致；尤其不向 `err_sink` 写任何字节，避免污染后续阶段预期的"占位 noop"语义。
- **后续阶段衔接预留**：未来实现 `declare variable=value` / `declare -p variable` 时，标准路径为：① 在 `builtins.rs` 新增 `pub fn run_declare(sink, err_sink, args, vars: &mut HashMap<String,String>)`，沿用 `run_complete` / `run_history` 的双 sink + 状态注入签名风格；② 在 `main.rs` 顶部 `use builtins::{... run_declare}` 追加；③ 把空 `{}` 替换为 `run_declare(...)` 调用并加 `if let Err(e) = ...` 包裹 IO 错误打印（沿用 `run_complete` arm 模板）。本阶段占位 arm 的位置和形态正是为该衔接路径量身定制。

## 目录结构

```
codecrafters-shell-rust/
├── src/
│   ├── builtins.rs   # [MODIFY] 第 18-20 行 BUILTINS 数组末尾追加 "declare"。
│   │                 #          唯一的 builtin 事实来源，追加后 run_type
│   │                 #          (第 173 行 BUILTINS.contains) 自动命中、
│   │                 #          completion.rs (第 402 行 BUILTINS 遍历)
│   │                 #          自动获得 TAB 候选。注释保持不变。
│   └── main.rs       # [MODIFY] 在 match cmd 表（第 272-496 行）中、
│                     #          _ => run_external(...) 兜底分支之前插入
│                     #          "declare" => {} 占位 arm，附 2-3 行注释
│                     #          说明本阶段占位、行为留待后续阶段，
│                     #          对齐既有 arm 的注释风格。
```

**不改动**：`completion.rs`、`exec.rs`、`redirect.rs`、`parser/*`、`tests/*`、`Cargo.toml`。

## Agent Extensions

### SubAgent

- **code-explorer**
- Purpose: 若后续阶段开始实现 `declare` 行为（变量存取 / `-p` 渲染 / 与 `env` / `set` / `export` 的关系等），用它一次性定位"shell 变量表应该挂载在 main 的 REPL 上下文哪个 `Rc<RefCell<...>>` 旁、是否需要新增模块"。本阶段单点改动小、无需调用。
- Expected outcome: 后续阶段产出精确的变量表挂载位置与生命周期分析，避免重复探索。