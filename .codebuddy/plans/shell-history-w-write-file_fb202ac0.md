---
name: shell-history-w-write-file
overview: 为 shell 的 `history` 内建新增 `history -w <path>` 子命令：将内存中的全部历史条目按行覆盖写入文件，文件末尾保留一个尾换行；与上阶段 `-r` 对称地在 dispatch 层实现，零改动 `run_history`。
todos:
  - id: impl-history-w
    content: 在 src/main.rs 的 "history" arm 中 -r 嗅探之后插入 -w 分支：收集历史 + File::create + BufWriter + writeln! + flush，静默失败 + continue
    status: completed
  - id: verify-regression
    content: 运行 cargo test 验证所有 run_history 单测 + 集成测试全绿，确认 -r / history / history N / 错误参数路径零回归
    status: completed
    dependencies:
      - impl-history-w
  - id: verify-e2e
    content: 手动端到端验证：启动 shell → 输入 echo hello / echo world / history -w /tmp/h → 退出后 cat 文件，断言三行历史 + 尾换行，与题面逐字节一致
    status: completed
    dependencies:
      - impl-history-w
---

## 用户需求

为 shell 实现 `history -w <path>` 内建子命令：将当前内存中的历史列表按时序**覆盖写入**到指定文件，使外部可以持久化本会话历史。

## 核心功能

- **`history -w <path>` 覆盖写文件**：按内存历史 1..N 顺序，逐行写入文件；不存在则创建，存在则覆盖（truncate）
- **`history -w` 自身已入栈**：复用 dispatch 前 `add_history_entry(line)` 流程，`history -w <path>` 这条命令在进入 arm 前已入栈，写出时是文件最后一条历史
- **尾换行**：每行以 `\n` 结尾（含最后一行），文件末尾呈现一个空行
- **静默错误处理**：文件创建/写入/flush 失败时静默忽略，不写 stderr、不阻断 REPL（与上阶段 `-r` 对称）
- **多余参数静默忽略**：`history -w <path> extra` 等价 `history -w <path>`
- **缺路径（仅 `-w`）静默 continue**
- **回归保护**：现有 `history` 无参 / `history N` / `history -r <path>` 行为完全不变；所有 `run_history` 单测全绿

## 视觉效果

`history -w <path>` 执行后**无任何 stdout/stderr 输出**。文件内容按时序逐行展开历史，最后一行是 `history -w <path>` 本身，文件末尾有一个尾换行（题面 `<|EMPTY LINE|>`）。

## 技术栈

延续现有 Rust + rustyline 14 技术栈，**无新增依赖**。`std::fs::File` + `std::io::BufWriter` + `writeln!` 宏均来自标准库。

## 实现策略

### 高层方案

唯一改动点是 `src/main.rs` 的 `"history"` arm，在上阶段 `-r` 嗅探分支之后、`run_history` 渲染路径之前，新增对称的 `-w` 嗅探分支：

1. **`-w` 路径**：嗅探 `args.first() == Some("-w")` → 取 `args.get(1)` 作 path → 复用现有 `editor.history()` + `History::get(idx, Forward)` 收集 owned `Vec<String>` → `File::create(path)` + `BufWriter` → 对每条 entry `writeln!(w, "{}", entry)` → `w.flush()` → `continue`
2. **现有路径**：未命中 `-w` 时保持当前代码（`-r` 嗅探 + `run_history` 渲染）100% 不变

为什么不动 `run_history`？（与 `-r` 决策对称）

- `run_history` 拿 `&[String]` 只读视图 + sink/err_sink，与「写文件」职责正交，混入会引入文件系统副作用，破坏 SRP
- 现有 11 个 `run_history` 单测契约是「输入 entries → 输出渲染格式」，混入 `-w` 会强行引入文件路径依赖，污染单测可达性
- `-w` 路径无 stdout/stderr 输出，与 `run_history` 的「渲染列表到 sink」职责完全不重叠

### 关键技术决策

- **`File::create` 而非 `OpenOptions::append`**：`-w` 是覆盖语义（题面期望「文件内容 = 当次会话精确历史」，不能保留旧内容）；`File::create` = `O_WRONLY|O_CREAT|O_TRUNC`，是 bash `-w` 的标准对应
- **`writeln!` 而非 `write!` + 手动 `\n`**：自动加 `\n`，覆盖「最后一行也要尾换行」需求，避免边界遗漏
- **`BufWriter` + 显式 `flush()`**：减少系统调用次数；Drop 时 flush 会静默吞错，显式 flush 是良好实践（即便仍走静默忽略策略，也保证错误路径明确）
- **静默错误用 `if let Ok(_) = ...` 链**：单一嵌套表达式覆盖「创建失败 / 写入失败 / flush 失败」全部失败模式，与 `-r` 静默风格对称
- **入栈顺序验证**：dispatch 前 `editor.add_history_entry(line)` 已把 `history -w <path>` 入栈 → 进入 arm 时它已是历史最末条 → 写入文件最后一行（在尾 `\n` 之前）正是它，逐字节匹配题面期望
- **`continue` 跳过下游**：`-w` 路径完成后直接 `continue` 主 REPL，不进入 `run_history` 渲染路径（保证无 stdout 输出）

### 性能与可靠性

- 时间复杂度：O(N) where N = 历史条目数；每条一次 `writeln!`（BufWriter 内部缓冲，摊销 O(1) 系统调用）
- 空间复杂度：O(N) 临时收集 `Vec<String>`（与 `run_history` 路径同等量级，无额外开销）
- 借用作用域严格收敛在 match arm 内，与 `-r` 路径 / jobs_table / completions 借用链零冲突
- 无 panic 路径：所有 `Result` 走 `Ok` 分支匹配，`None` 走默认跳过

## 实现注意事项

- **嗅探位置**：放在 `-r` 嗅探之后、`run_history` 渲染之前，保持「`-r` → `-w` → 渲染」自上而下的可读性
- **写出顺序**：用 `SearchDirection::Forward` 逐项 `get`，与上阶段 `-r` 读入和现有渲染路径完全同构，零认知负担
- **不引入 `std::fs::OpenOptions`**：`File::create` 已足够（覆盖语义）；引入 OpenOptions 反而增加误用风险
- **不递归创建目录**：题面只要求文件不存在时创建，未要求父目录递归创建；若父目录不存在则按静默失败策略 `continue`
- **绝对路径假设**：题面 tester 用绝对路径，无需 `~` / 相对路径展开
- **回归覆盖**：现有 11 个 `run_history` 单测 + 上阶段 `-r` 端到端行为必须全绿；本阶段可不新增单测，依靠 codecrafters 官方 tester 端到端验证 + 一次手动 e2e 验证

## 架构设计

延续既有「REPL 主循环 dispatch + 内建函数纯函数化」分层，与上阶段 `-r` 完全对称：

- `main.rs`：REPL 循环 + 内建分发；本阶段新增 `-w` 嗅探，仅在 `"history"` arm 内部
- `builtins.rs::run_history`：纯函数化历史渲染，本阶段零改动

数据流（仅 `-w` 命中分支）：

```
用户输入 → editor.readline() → add_history_entry(原始命令行)
       → parse → dispatch → "history" arm
       → 嗅探 args[0]=="-w" → editor.history() 收集 Vec<String>
       → File::create(args[1]) → BufWriter → 逐行 writeln! → flush → continue
```

## 修改文件清单

```
project-root/
└── src/
    └── main.rs   # [MODIFY] 仅在 "history" arm 的 -r 嗅探分支之后、run_history 渲染之前，
                  #   插入对称的 -w 嗅探分支：
                  #   1. 嗅探 args[0]=="-w" → 取 args.get(1) 作 path
                  #   2. 复用 editor.history() + History::get(idx, Forward) 收集 Vec<String>
                  #      （与下方渲染路径用同样收集方式，避免认知分裂）
                  #   3. File::create(path) + BufWriter::new + 逐行 writeln!(w, "{}", entry)
                  #      + w.flush()，全程 if let Ok 静默失败
                  #   4. 完成后 continue（跳过下方 run_history 渲染路径，无 stdout 输出）
                  #   5. 现有 -r 嗅探（第 250~263 行）与 run_history 渲染（第 282~291 行）保持原样
```

注：`src/builtins.rs` 零改动；不新增 tests 文件（依靠 codecrafters 官方 tester + 手动 e2e 验证）。

## 关键代码结构

`main.rs` 内 `"history"` arm 新增的 `-w` 嗅探片段（接口契约示意）：

```rust
// 紧跟 -r 嗅探分支之后
if args.first().map(|s| s.as_str()) == Some("-w") {
    if let Some(path) = args.get(1) {
        // 复用现有收集方式
        let h = editor.history();
        let mut entries: Vec<String> = Vec::with_capacity(h.len());
        for i in 0..h.len() {
            if let Ok(Some(sr)) = h.get(i, SearchDirection::Forward) {
                entries.push(sr.entry.into_owned());
            }
        }
        if let Ok(file) = std::fs::File::create(path) {
            use std::io::Write;
            let mut w = std::io::BufWriter::new(file);
            for entry in &entries {
                let _ = writeln!(w, "{}", entry);
            }
            let _ = w.flush();
        }
    }
    continue;
}
```