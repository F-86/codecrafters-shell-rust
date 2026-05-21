---
name: history-limit-n-stage
overview: "扩展 run_history 支持可选参数 N：`history N` 只显示最近 N 条且保留全局连续编号；非数字参数对齐 bash 报 `history: <arg>: numeric argument required`；多余参数静默忽略只取第一个。"
todos:
  - id: extend-run-history-arg-parsing
    content: 扩展 src/builtins.rs::run_history 函数体：解析 args[0] 为 usize，用 saturating_sub 算 start，遍历 entries[start..] 时编号用 start+i+1；非法参数走 err_sink + 早返回 Ok
    status: completed
  - id: update-run-history-doc
    content: 更新 run_history 文档注释：补「参数语义」小节（n=0 / n>=len / 非数字 / 多余参数四种 case）+「编号 = 全局下标 + 1」契约说明
    status: completed
    dependencies:
      - extend-run-history-arg-parsing
  - id: add-history-n-unit-tests
    content: "在 builtins.rs 的 #[cfg(test)] mod tests 新增 invoke_history_with_args 辅助函数 + 6 个单测：全局编号语义、n=0、n>=len、n=len、非数字、负数、多余参数"
    status: completed
    dependencies:
      - extend-run-history-arg-parsing
  - id: verify-build-and-e2e
    content: 运行 cargo build / cargo test 验证零 warning + 全绿，并端到端手测 history 2 / history 0 / history 99 / history abc / 无参数 history 五种 case
    status: completed
    dependencies:
      - add-history-n-unit-tests
      - update-run-history-doc
---

## 用户需求

为 shell 的 `history` 内建增加 `history <n>` 参数能力，显示最近 n 条历史命令。

## 核心功能

- **`history <n>` 显示末尾 n 条**：当 n < 历史总数时，输出最后 n 条；当 n >= 历史总数时，等价无参数全列；当 n = 0 时输出 0 字节
- **编号保持全局连续**（bash 语义）：即使只显示末尾 n 条，每行的编号仍是该条目在完整历史中的位置（如 4 条历史 + `history 2` → 输出编号 `3` 和 `4`，不是 `1` 和 `2`）
- **`history` 命令自身入栈**：保持上阶段行为，`history 2` 自己也出现在末行
- **非法参数错误处理**（对齐 bash）：`history abc` / `history -5` 等非数字 / 负数 → 向 stderr 输出 `history: <arg>: numeric argument required`，不中断 REPL
- **多余参数静默忽略**（对齐 bash）：`history 2 foo bar` 等价 `history 2`，仅取第一个参数解析

## 视觉效果

输出格式与上阶段完全一致：每行 `{:>4}  {entry}` —— 编号右对齐 4 字符宽 + 2 空格分隔 + 命令原文 + 换行；编号 ≥ 10000 时按实际宽度输出。无参数 `history` 行为与上阶段保持回归一致。

## 技术栈

延续现有 Rust + rustyline 14 技术栈，本阶段**无新增依赖**。

## 实现策略

### 高层方案

唯一改动点是 `src/builtins.rs::run_history` 函数体内部展开——签名、调用方（main.rs dispatch）、`BUILTINS` 数组、REPL 历史接线均保持不动。本阶段在上阶段铺设的 `args: &[String]` 参数位上展开解析逻辑：

1. **参数解析**：`args.first()` 取首参数尝试 `parse::<usize>()`，失败走 stderr 错误路径并早返回 `Ok(())`（不阻断 REPL）；缺省时 `n = entries.len()` 等价全列
2. **窗口截取**：`start = entries.len().saturating_sub(n)` 一行覆盖 n=0 / n>=len / n<len 三种 case，无需分支
3. **编号渲染**：遍历 `entries[start..]` 时编号用 `start + i + 1`（**全局下标 + 1**，本阶段最易写错点）

### 关键技术决策

- **`saturating_sub` 而非 `if/else`**：单一表达式覆盖所有数值边界，可读性 + 健壮性双优
- **`usize::from_str` 而非自定义解析**：自动覆盖负数（`-5` 解析失败）、非数字、空字符串等异常输入，与 bash「numeric argument required」语义天然对齐
- **err_sink 写失败用 `let _ =` 吞掉**：沿用 `run_jobs` / `run_cd` 既有风格，避免与「非法参数」语义重叠的双重输出（若 return Err，main.rs 会再 `eprintln! shell: write error`）
- **签名零改动**：`_args` / `_err_sink` 上阶段已铺设的参数位本阶段直接启用，调用方零感知

### 性能与可靠性

- 时间复杂度 `O(min(n, len))`：仅遍历输出窗口，不复制 entries
- 空间复杂度 `O(1)` 额外（writeln! 流式写入 sink）
- 无 borrow / lifetime / pipeline / 重定向风险——纯函数体改造

## 实现注意事项

- **编号陷阱**：必须用 `start + i + 1`，**不可** 用 `i + 1`——单测 #1 显式断言此点防止回归
- **错误格式精确**：`history: {arg}: numeric argument required\n`（**不带** `bash:` / `shell:` 前缀，与项目其他 builtin 错误风格一致；末尾 `\n` 由 `writeln!` 自带）
- **回归保护**：上阶段 4 个单测（空表 / 单条 / 多条 / 12 条宽度）必须继续全绿，验证无参数路径不被破坏
- **辅助函数扩展**：现有 `invoke_history(entries)` 硬编码 `args=&[]`，需新增 `invoke_history_with_args(entries, args)` 变体支持本阶段 args 注入

## 修改文件清单

```
project-root/
├── src/
│   └── builtins.rs   # [MODIFY] 仅 run_history 函数体改动 + 文档注释更新 + 追加单测
│                     #   1. 函数体：args 解析 → 窗口截取 → 全局编号渲染
│                     #   2. 文档注释：补「参数语义」小节（0/>=len/非数字/多余参数四种 case）+「编号 = 全局下标 + 1」契约
│                     #   3. 函数签名：_args → args、_err_sink → err_sink 去掉下划线
│                     #   4. #[cfg(test)] mod tests：新增 invoke_history_with_args 辅助 + 6 个单测
│                     #      覆盖：编号语义 / n=0 / n>=len / n=len / 非数字 / 负数 / 多余参数
│                     #      原 4 个单测保持不动（回归保护）
└── src/main.rs       # [NO CHANGE] dispatch 已透传 args，零改动
```

## 关键代码结构

仅 `run_history` 函数体核心逻辑（接口契约，非完整实现）：

```rust
pub fn run_history(
    sink: &mut dyn Write,
    err_sink: &mut dyn Write,
    args: &[String],
    entries: &[String],
) -> io::Result<()>;
// 语义契约：
// - args 为空 → 全列出，编号 1..=len
// - args[0] 解析 usize 成功（值为 n）→ 输出末尾 min(n, len) 条，编号 = 全局下标 + 1
//   - n = 0 → 0 字节输出
//   - n >= len → 等价无参数全列
//   - args[1..] 静默忽略
// - args[0] 解析失败 → err_sink 写 "history: {arg}: numeric argument required\n"，返回 Ok(())
```