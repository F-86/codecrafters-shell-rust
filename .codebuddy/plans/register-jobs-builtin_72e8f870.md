---
name: register-jobs-builtin
overview: 注册 `jobs` 为 shell builtin，提供空实现：`type jobs` 报告为 builtin、`jobs` 命令本身无任何输出。
todos:
  - id: register-jobs-builtin
    content: 在 `src/builtins.rs` 的 `BUILTINS` 数组末尾追加 `"jobs"`，并新增 `pub fn run_jobs(sink, err_sink, args) -> io::Result
    status: completed
---

## 产品概述

为 Rust 实现的 shell 注册 `jobs` 内建命令。本阶段仅完成「注册 + 空实现」，真实的后台任务列表逻辑留待后续阶段。

## 核心功能

- `type jobs` 输出 `jobs is a shell builtin`（走 stdout，可被 `1>` / `>` 重定向）
- 直接执行 `jobs` 时不产生任何输出（stdout 与 stderr 均为空），随即回到提示符
- 维持现有 REPL 主循环、错误信道分离、写错误传播等既有约定

## 技术栈

- 语言：Rust（沿用现有 `Cargo.toml` 工程）
- I/O 抽象：复用现有 `&mut dyn Write` sink / err_sink 模式
- 行编辑：`rustyline`（无需改动）

## 实现策略

最小侵入式接入：把 `"jobs"` 追加到 `BUILTINS` 单一数据源，新增 `run_jobs` 空实现函数，在 `main.rs` 的 dispatch `match` 中加一条分支即可，不引入任何新模块或新依赖。

### 关键技术决策

1. **复用 `BUILTINS` 单一数据源**：`run_type` 判定「是否为 shell builtin」的唯一依据是 `BUILTINS.contains(&target)`（`src/builtins.rs:111`），故注册仅需在 `BUILTINS`（`src/builtins.rs:17`）末尾追加 `"jobs"`，`type jobs` 测试自动通过；同时 TAB 补全候选源若基于该常量可一并受益。
2. **保留与既有 builtin 一致的签名**：`run_jobs(sink, err_sink, args) -> io::Result<()>`，本阶段函数体直接 `Ok(())`。这样后续阶段填充真实任务列表逻辑时，dispatch 框架无需任何改动，blast radius 为零。
3. **dispatch 写错误传播模式对齐**：用 `if let Err(e) = run_jobs(...) { eprintln!("shell: write error: {}", e); }`，与 `echo` / `pwd` / `type` / `complete` 分支风格保持一致。
4. **不在本阶段引入 job 表数据结构**：题面 Notes 明确「列表实现留待后续阶段」。提前引入 `Rc<RefCell<Vec<Job>>>` 等会污染 `complete` registry 的成熟模式且无验证用例，违反 YAGNI。

### 性能与可靠性

- 热路径无新增开销：dispatch `match` 增加一个常量分支为 O(1)；`BUILTINS.contains` 是线性扫描，常量长度仅 7，可忽略。
- 空实现无 I/O，无错误来源；`io::Result<()>` 仅为签名占位，运行时永远 `Ok(())`。
- 向后兼容：未触碰 `parser` / `exec` / `redirect` / `completion`，零回归风险。

## 实现注意事项

- 不要修改 `BUILTINS` 已有顺序（避免影响 TAB 补全显示顺序的潜在视觉差异），追加在末尾。
- `run_jobs` 即便函数体为空，也要正式接收 `sink` 与 `err_sink` 参数（用 `_` 前缀消除 unused 警告），保持后续无缝扩展。
- 不在 dispatch 中写 `_ = args;`、不引入 `#[allow(unused)]`：调用约定与既有分支完全对齐。
- 在 `run_jobs` 上方加中文 doc comment，说明：本阶段空实现 + 后续将填充任务列表 + 错误信道对齐既有 builtin。

## 目录结构

仅修改两个文件，不新增/删除文件。

```
codecrafters-shell-rust/
└── src/
    ├── builtins.rs   # [MODIFY] (1) BUILTINS 末尾追加 "jobs"，使 type jobs 命中 builtin 分支
    │                 #          (2) 新增 pub fn run_jobs(sink, err_sink, args) -> io::Result<()>
    │                 #              本阶段函数体直接返回 Ok(())；doc comment 说明空实现意图
    │                 #              与后续阶段扩展点（任务表注入位置）
    └── main.rs       # [MODIFY] (1) use 行新增导入 run_jobs
                      #          (2) 在 match cmd 的 "complete" 与 _ 之间新增 "jobs" 分支，
                      #              沿用 if let Err(e) = run_jobs(...) { eprintln!(...) } 模式
```

## 验证策略

- `type jobs` → stdout 严格等于 `jobs is a shell builtin\n`
- `jobs` → stdout 与 stderr 均为空，REPL 正常返回 `$ ` 提示符
- 既有 stage 测试（echo / pwd / cd / type / complete / 重定向）全部保持绿色