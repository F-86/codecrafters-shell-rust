---
name: shell-history-r-read-file
overview: 为 shell 的 `history` 内建新增 `history -r <path>` 子命令：从文件按行追加历史条目到 rustyline Editor 的内部历史列表，空行跳过，文件读取失败静默忽略。
todos:
  - id: impl-history-r
    content: 在 src/main.rs 的 "history" arm 前置 -r 嗅探：File::open + BufRead::lines() 逐行 add_history_entry，空行跳过、错误静默、完成 continue
    status: completed
  - id: verify-regression
    content: 运行 cargo test 验证 11 个 run_history 单测全绿，确认无参 / history N / 错误参数路径零回归
    status: completed
    dependencies:
      - impl-history-r
  - id: verify-e2e
    content: 手动端到端验证：写临时文件含 "echo hello\necho world\n\n" → 启动 shell → 输入 history -r path → 输入 history → 断言输出 4 行编号 1~4 与题面逐字节一致
    status: completed
    dependencies:
      - impl-history-r
---

## 用户需求

为 shell 实现 `history -r <path>` 内建子命令：从指定文件读取历史条目并**追加**到内存中的历史列表，使后续 `history` 命令能够看到这些条目。

## 核心功能

- **`history -r <path>` 读文件追加历史**：逐行读取文件内容，每行作为一条历史条目追加到 rustyline 内部历史栈，无任何 stdout 输出
- **空行跳过**：文件中的空行不入栈（对齐 bash 行为，避免污染编号）
- **`history -r` 命令自身已入栈**：复用 dispatch 前 `add_history_entry(line)` 流程，`history -r <path>` 这条命令会先于文件条目入栈，与题面期望编号匹配
- **静默错误处理**：文件不存在 / 无读取权限时静默忽略，不写 stderr、不阻断 REPL
- **多余参数静默忽略**：`history -r <path> extra` 等价 `history -r <path>`，仅取 `args[1]` 作路径
- **回归保护**：现有 `history` 无参 / `history N` / 错误参数 行为完全不变

## 视觉效果

`history -r <path>` 执行后无任何输出（无 stdout、无 stderr）。下一次 `history` 命令仍按原有格式 `{:>4}  {entry}` 输出，包含追加的所有条目，编号从全局位置 1 开始连续递增。

## 技术栈

延续现有 Rust + rustyline 14 技术栈，**无新增依赖**。`std::fs::File` + `std::io::BufRead`/`BufReader` 均来自标准库。

## 实现策略

### 高层方案

唯一改动点是 `src/main.rs` 的 `"history"` dispatch 分支。改造为**双路径分发**：

1. **`-r` 路径**：在进入 `run_history` 调用之前先嗅探 `args.first() == Some("-r")`，命中则取 `args.get(1)` 作为路径，打开文件、`BufRead::lines()` 逐行迭代、跳过空行、对每行调 `editor.add_history_entry(line)`，**完成后 `continue`**，不进入 `run_history`、不产生任何输出
2. **现有路径**：未命中 `-r` 时保持当前代码 100% 不变（收集 entries → 调 `run_history`）

为什么不动 `run_history`？

- `run_history` 拿到的是 `&[String]` 只读视图，无法向 `Editor` 写入
- `-r` 路径无 stdout/stderr 输出，与 `run_history`「渲染历史列表」职责正交，混入只会破坏 SRP
- 现有 7 个 `run_history` 单测全部基于「输入 entries → 输出格式断言」契约，混入 `-r` 路径会强行引入 Editor 类型依赖，污染测试可达性

### 关键技术决策

- **借用顺序无冲突**：`-r` 路径只需要 `editor.add_history_entry(&mut self, ...)`，与现有路径中的 `editor.history(&self)` 是**串行不同分支**，编译器借用检查零冲突
- **`BufRead::lines()` 而非 `read_to_string` + `split('\n')`**：流式读取、O(1) 额外内存、自动剥离 `\n`/`\r\n`，与 bash 行为一致
- **静默错误用 `let Ok(file) = File::open(...) else { continue; }`**：单一 let-else 表达式覆盖「文件不存在 / 无权限 / 路径无效」全部失败模式；单行读失败（`Result<String>`）同样静默 `if let Ok(s)` 跳过
- **空行判断用 `s.is_empty()`**：`BufRead::lines()` 已剥离行尾换行，纯空字符串即原文空行
- **多余参数静默**：仅取 `args.get(1)`，`args[2..]` 不读取（与 `history N extra` 风格一致）
- **`continue` 而非 fallthrough**：`-r` 路径完成后直接 `continue` 主 REPL 循环，跳过所有后续逻辑（包括 sink/err_sink 已打开但本路径无需写入——sink 在分支匹配前已构造，但其生命周期管理由 Box 自动 drop 处理，不影响正确性）

### 性能与可靠性

- 时间复杂度：O(L) where L = 文件行数；每行一次 `add_history_entry`（rustyline 内部 VecDeque push，O(1) 摊销）
- 空间复杂度：O(1) 额外（BufReader 内部 buffer + 单行 owned String 临时变量，无累计 Vec）
- 借用作用域严格收敛在 match arm 内，不影响外层 `jobs_table` / `completions` 借用链
- 无 panic 路径：所有 `Result` 走 `Ok` 分支匹配，`None` 走默认跳过

## 实现注意事项

- **嗅探放在 sink/err_sink 之前还是之后？**：必须放在 `match cmd` 之后、当前 `"history"` arm 内的最前面；sink/err_sink 已在 arm 之前打开（main.rs 第 166~183 行），但 `-r` 路径不使用它们——Box 在 arm 结束时正常 drop，无资源泄漏
- **入栈顺序验证**（题面编号 1~4 匹配的关键）：

1. 用户输入 `history -r <path>` → main.rs 第 118 行 `add_history_entry` 入栈（编号 1）
2. 进入 `"history"` arm → 嗅探 `-r` 命中 → 读文件追加 `echo hello`（编号 2）、`echo world`（编号 3）
3. 用户输入 `history` → `add_history_entry` 入栈（编号 4）→ 进入 arm → 走现有 `run_history` 路径输出 4 行

- **路径缺失（仅 `-r` 无第二参）**：`args.get(1)` 返回 `None`，按静默忽略策略 `continue`，不报错（题面未规定，对齐 q2）
- **绝对路径假设**：题面 tester 用绝对路径，无需做 `~` / 相对路径基目录展开；`File::open` 直接传字符串路径即可
- **回归测试覆盖范围**：现有 11 个 `run_history` 相关单测必须全绿；本阶段在 `tests/` 目录可新增端到端测试覆盖 `-r` 行为，但 `src/builtins.rs::tests` 不动

## 架构设计

延续现有「REPL 主循环 dispatch + 内建函数纯函数化」分层：

- `main.rs`：REPL 循环 + 内建分发；本阶段新增 `-r` 嗅探仅在 `"history"` arm 内部
- `builtins.rs::run_history`：纯函数化历史渲染，本阶段零改动

数据流（仅 `-r` 命中分支）：

```
用户输入 → editor.readline() → add_history_entry(原始命令行) 
       → parse → dispatch → "history" arm 
       → 嗅探 args[0]=="-r" → File::open(args[1]) → BufReader::lines() 
       → 逐行 add_history_entry(非空行) → continue
```

## 修改文件清单

```
project-root/
├── src/
│   └── main.rs        # [MODIFY] 仅 "history" match arm 改动
│                      #   1. 在现有 entries 收集逻辑之前，新增 args[0]=="-r" 嗅探分支
│                      #   2. -r 命中：File::open + BufReader::lines() + 逐行 add_history_entry
│                      #      (空行跳过、错误静默、多余参数忽略)，完成后 continue
│                      #   3. -r 未命中：保持现有 230~258 行代码 100% 不变
│                      #   4. 新增 use std::fs::File 与 std::io::BufRead 引入
└── tests/
    └── history_r.rs   # [NEW] 端到端集成测试（可选，复刻 tests/ 目录现有风格）
                       #   场景：写临时历史文件 → 启动 shell → 发送 history -r → 发送 history
                       #   断言：stdout 编号 1~N 包含 -r 命令自身 + 文件条目 + history 命令
                       #   如已有 tests/ 目录通用框架，沿用其 spawn/assert 工具
```

注：`tests/history_r.rs` 是否新增由现有 `tests/` 目录测试组织风格决定；若现有集成测试覆盖度足够（codecrafters tester 已是端到端），可跳过本文件、仅依靠官方 tester 验证。

## 关键代码结构

仅 `main.rs` 内 `"history"` arm 的新增嗅探片段（接口契约示意，非完整实现）：

```rust
"history" => {
    // 新增：-r <path> 子命令嗅探
    if args.first().map(|s| s.as_str()) == Some("-r") {
        if let Some(path) = args.get(1) {
            if let Ok(file) = std::fs::File::open(path) {
                use std::io::BufRead;
                let reader = std::io::BufReader::new(file);
                for line in reader.lines().flatten() {
                    if !line.is_empty() {
                        let _ = editor.add_history_entry(line);
                    }
                }
            }
        }
        continue;
    }
    // 既有路径：entries 收集 + run_history 调用（保持原样）
    let h = editor.history();
    // ... (原 230~258 行不变)
}
```