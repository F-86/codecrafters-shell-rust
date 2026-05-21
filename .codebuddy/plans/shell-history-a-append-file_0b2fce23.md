---
name: shell-history-a-append-file
overview: "为 shell 的 `history` 内建新增 `history -a <path>` 子命令：将「自上次 -a 之后新产生的会话历史」追加写入文件，文件末尾保留尾换行；引入会话级游标 `last_appended_len: usize` 记录上次追加点，与 `-r`/`-w` 在 dispatch 层对称实现，零改动 `run_history`。"
todos:
  - id: impl-history-a
    content: 在 src/main.rs 主循环外新增 last_appended_len 游标，并在 -w 嗅探之后插入 -a 嗅探分支：切片收集 + OpenOptions::append + writeln! + flush + 推进游标
    status: completed
  - id: verify-regression
    content: 运行 cargo test 验证所有 run_history 单测 + 集成测试全绿，确认 -r / -w / history / history N 路径零回归
    status: completed
    dependencies:
      - impl-history-a
  - id: verify-e2e
    content: 手动端到端验证两轮：(1) 预写 initial_command_1/2 → echo new_command → history -a → cat 断言四行 + 尾换行；(2) 再 echo foo → history -a → 断言仅追加两行，无重复
    status: completed
    dependencies:
      - impl-history-a
---

## 用户需求

为 shell 实现 `history -a <path>` 内建子命令：将「自上次 `-a` 之后」内存中新增的历史条目按时序追加写入到指定文件，使外部可以增量持久化本会话历史。

## 核心功能

- **`history -a <path>` 增量追加写文件**：仅写出自上次 `-a` 之后新增的历史条目，原文件已有内容保持不变（不截断）
- **首次 `-a`**：写出当前内存中的全部历史（包括 `history -a <path>` 自身）
- **多次 `-a` 增量语义**：第二次及后续 `-a` 只追加自上次 `-a` 后输入的命令（含本次 `-a` 自身），不重复追加
- **`history -a` 自身已入栈**：复用 dispatch 前 `add_history_entry(line)` 流程，`history -a <path>` 作为追加内容的最后一行
- **文件不存在时创建**：以追加方式打开（`O_WRONLY|O_CREAT|O_APPEND`）
- **尾换行**：每行以 `\n` 结尾（含最后一行），文件末尾呈现一个空行
- **静默错误处理**：文件打开 / 写入 / flush 失败时静默忽略，不写 stderr、不阻断 REPL，下次 `-a` 仍尝试本批
- **多余参数静默忽略**：`-a <path> extra` 等价 `-a <path>`
- **缺路径**：`args.get(1)` 返回 None，静默 continue，不推进游标
- **回归保护**：现有 `history` 无参 / `history N` / `history -r` / `history -w` 行为完全不变

## 视觉效果

`history -a <path>` 执行后**无任何 stdout/stderr 输出**。文件内容呈现：原有内容（tester 预写或上次 `-a` 已追加部分）保留不变，紧随其后是本次新追加的若干行，每行带尾换行，最末行（`history -a <path>` 自身）之后也有 `\n`，等价题面 `<|EMPTY LINE|>`。

## 技术栈

延续现有 Rust + rustyline 14 技术栈，**无新增依赖**。`std::fs::OpenOptions` + `std::io::BufWriter` + `writeln!` 宏均来自标准库。

## 实现策略

### 高层方案

两处改动，均在 `src/main.rs`：

1. **主循环外新增会话级游标变量** `let mut last_appended_len: usize = 0;`，记录「上次 `-a` 成功打开文件时 `editor.history().len()` 的值」
2. **在 `"history"` arm 的 `-w` 嗅探之后、`run_history` 渲染之前**，对称插入 `-a` 嗅探分支：

- 嗅探 `args.first() == Some("-a")` → 取 `args.get(1)` 作 path
- `let total = editor.history().len()`；`let start = last_appended_len.min(total)`（防御性 clamp）
- 用 `History::get(idx, Forward)` 收集切片 `[start, total)` 为 owned `Vec<String>`
- `OpenOptions::new().create(true).append(true).open(path)` + `BufWriter` + 逐行 `writeln!` + `flush`
- 文件**成功打开**后即推进 `last_appended_len = total`（与 bash 一致：失败不回滚以避免重复写）
- `continue`

### 为什么不动 `run_history`？（与 `-r` / `-w` 决策对称）

- 增量追加写文件与「stdout 渲染」职责正交；`run_history` 拿 `&[String]` 只读视图无需也不应感知文件系统或游标状态
- 现有 11 个 `run_history` 单测契约是「输入 entries → 输出渲染格式」，混入游标会破坏 SRP 和单测可达性
- 游标变量本质是「main 循环的会话级状态」，与 `completions` / `jobs_table` 同层级，自然归属 `main.rs`

### 关键技术决策

- **`OpenOptions::append` 而非 `File::create`**：`File::create` = `O_TRUNC` 会清空 tester 预写的 initial_command_1/2，破坏题面期望；`OpenOptions::new().create(true).append(true)` = `O_WRONLY|O_CREAT|O_APPEND`，是 bash `-a` 的标准对应
- **游标用 `usize` 而非 `Option<usize>`**：初值 0 语义清晰（「从头追加」），与 `History::len()` 返回值同型避免 cast
- **游标更新放在 `if let Ok(file)` 成功分支内、`flush()` 之后**：意图明确「文件成功打开且尝试写入后推进」；写入 / flush 失败不回滚，与 bash 一致（避免下次重复写同一批）
- **`start = last_appended_len.min(total)`**：理论上不可能越界（每次只增），但 rustyline 14 内部 `ignore_dups` 等机制可能导致 `len()` 收缩，廉价的 robustness 防止 panic
- **不在 `-r` 命中后同步推进游标**：题面 tester 不覆盖「先 `-r` 再 `-a`」，保持最小改动，未来扩展点用注释标注
- **`writeln!` 自动加 `\n`**：覆盖「末行也要尾换行」需求，与 `-w` 完全对称

### 性能与可靠性

- 时间复杂度：O(N) where N = 自上次 `-a` 后新增条目数；多次 `-a` 摊销总写 O(M)，M = 全部历史条目
- 空间复杂度：O(N) 切片 `Vec<String>`，与 `-w` 全量收集相比更优
- 借用作用域严格收敛在 match arm 内，与 `-r` / `-w` / jobs_table 借用链零冲突
- 无 panic 路径：`.min(total)` 防御越界，所有 `Result` 走 `if let Ok` 静默忽略
- 游标单调递增、单线程同步访问，无并发风险

## 实现注意事项

- **游标变量插入位置**：放在主循环外（建议紧邻 `jobs_table` 创建附近），保证跨 REPL 循环存活
- **嗅探分支顺序**：`-r` → `-w` → `-a` → 渲染，对应代码可读性「读 → 全量写 → 增量写 → 列出」自然递进
- **入栈顺序验证**：dispatch 前 `add_history_entry(line)` 已把 `history -a <path>` 入栈 → 进入分支时它已是 entries[total-1]，切片末位正是它，与题面期望逐字节匹配
- **绝对路径假设**：题面 tester 用绝对路径，无需 `~` / 相对路径展开
- **回归覆盖**：现有 11 个 `run_history` 单测 + `-r` / `-w` 端到端行为必须全绿；本阶段依靠 codecrafters 官方 tester 端到端验证 + 手动 e2e 双重验证（含「多次 `-a`」场景）

## 架构设计

延续既有「REPL 主循环 dispatch + 会话级状态变量 + 内建函数纯函数化」分层，与上阶段 `-r` / `-w` 完全对称：

- `main.rs`：REPL 循环 + 内建分发 + 会话级游标 `last_appended_len`
- `builtins.rs::run_history`：纯函数化历史渲染，本阶段零改动

数据流（仅 `-a` 命中分支）：

```
用户输入 → editor.readline() → add_history_entry(原始命令行)
       → parse → dispatch → "history" arm
       → 嗅探 args[0]=="-a"
       → total = editor.history().len()
       → 切片 [last_appended_len .. total) 收集 Vec<String>
       → OpenOptions::append.open(args[1]) → BufWriter → 逐行 writeln! → flush
       → last_appended_len = total → continue
```

## 修改文件清单

```
project-root/
└── src/
    └── main.rs   # [MODIFY] 两处改动：
                  #   1. 主循环外（jobs_table 附近）新增会话级游标变量：
                  #      let mut last_appended_len: usize = 0;
                  #      跨 REPL 循环存活，无需 Rc<RefCell<>>（单线程同步访问，
                  #      不跨闭包、不共享给 helper）
                  #   2. "history" arm 中 -w 嗅探（第 290~311 行）之后、run_history
                  #      渲染（第 313 行起）之前，插入对称的 -a 嗅探分支：
                  #      - 嗅探 args[0]=="-a" → 取 args.get(1) 作 path
                  #      - total = editor.history().len()
                  #      - start = last_appended_len.min(total) 防御性 clamp
                  #      - History::get(idx, Forward) 收集切片 [start..total) 为 Vec<String>
                  #      - OpenOptions::new().create(true).append(true).open(path)
                  #        + BufWriter::new + 逐行 writeln! + w.flush()，全程 if let Ok 静默失败
                  #      - 文件成功打开后推进 last_appended_len = total（不论后续写入是否成功）
                  #      - continue 跳过下方 run_history 渲染路径
                  #   3. 现有 -r 嗅探（第 250~263 行）、-w 嗅探（第 290~311 行）、
                  #      run_history 渲染（第 313~339 行）保持原样
```

注：`src/builtins.rs` 零改动；不新增 tests 文件（依靠 codecrafters 官方 tester + 手动 e2e 验证含多次 `-a`）。

## 关键代码结构

`main.rs` 主循环外新增游标变量（紧邻 `jobs_table` 创建附近）：

```rust
// 「history -a」会话级游标：记录上次 -a 成功打开文件时 editor.history().len()。
// 下次 -a 仅追加 history[last_appended_len..] 切片，实现 bash 增量追加语义。
// 单线程 REPL 串行访问，无需 Rc<RefCell<>>。
let mut last_appended_len: usize = 0;
```

`"history"` arm 内 `-a` 嗅探分支（紧跟 `-w` 之后）：

```rust
if args.first().map(|s| s.as_str()) == Some("-a") {
    if let Some(path) = args.get(1) {
        let h = editor.history();
        let total = h.len();
        // 防御性 clamp：rustyline 内部 ignore_dups 等机制可能导致 len 收缩
        let start = last_appended_len.min(total);
        let mut entries: Vec<String> = Vec::with_capacity(total - start);
        for i in start..total {
            if let Ok(Some(sr)) = h.get(i, SearchDirection::Forward) {
                entries.push(sr.entry.into_owned());
            }
        }
        if let Ok(file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            use std::io::Write;
            let mut w = std::io::BufWriter::new(file);
            for entry in &entries {
                let _ = writeln!(w, "{}", entry);
            }
            let _ = w.flush();
            // 文件成功打开即推进游标：写入/flush 失败不回滚（与 bash 一致，
            // 避免重复写同一批）；缺路径 / 打开失败时不推进，下次 -a 仍尝试本批。
            last_appended_len = total;
        }
    }
    continue;
}
```