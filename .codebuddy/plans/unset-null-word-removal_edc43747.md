---
name: unset-null-word-removal
overview: 实现 bash「null word removal」语义：unquoted 展开后整个 word 完全为空时，丢弃该 word（不进 argv）。仅修改 tokenize 层 Normal 态两处未命中分支，移除「未命中无条件 in_token=true」的副作用，并更新一项受影响测试。
todos:
  - id: update-tokenize-null-word-removal
    content: 在 src/parser/tokenize.rs Normal 态 `${NAME}` 与 `$NAME` 两个子分支，把无条件 `in_token = true` 改为仅命中（`vars.get(&name).is_some()`）时设置；同步修订注释明确 null word removal 语义
    status: completed
  - id: update-module-doc
    content: 在 src/parser/mod.rs 模块头注释追加一句，描述 unquoted 未命中全空 word 丢弃的语义边界
    status: completed
    dependencies:
      - update-tokenize-null-word-removal
  - id: update-and-add-tests
    content: 在 src/parser/tests.rs 重命名并更新 `dollar_expansion_unquoted_miss_is_empty_token`，新增 7 条覆盖题面 verbatim、`${UNSET}` 单独成词、连续未命中、命中空值、引号片段保留 token 等边界用例
    status: completed
    dependencies:
      - update-tokenize-null-word-removal
  - id: verify-build-and-e2e
    content: 运行 cargo build / cargo test 确认零警告零回归；通过 printf 注入题面 verbatim 序列（`declare existing=existingsvalue` + 调用外部脚本）端到端核验子进程 argv 仅含 `["program", "end", "existingsvalue"]`
    status: completed
    dependencies:
      - update-and-add-tests
      - update-module-doc
---

## Product Overview

为当前 Rust 实现的 shell 添加「未设置变量在 unquoted 展开为空时丢弃整个 word」的语义（bash 称作 null word removal），与已有的 `$VAR` / `${VAR}` 展开能力对齐。

## Core Features

- **未命中变量空展开 + word 丢弃**：unquoted 形态下若整个 word 完全由「未命中变量展开」贡献且最终为空，整个 word 不进入 argv（题面示例 `${missing2}` 完全消失，不产生第三个参数）。
- **字面字符 / 引号片段保留为 word 触发器**：`${UNSET}end` 仍产出 `end`、`${UNSET}""` 仍产出空 token、`""${UNSET}` 同理；显式引号 / 字面字符 / 转义字符 / 命中变量都能开启 word。
- **双引号内未命中行为不变**：`"$UNSET"` / `"${UNSET}"` 仍保留为空 token，对齐 bash「双引号内展开不触发 null word removal」。
- **命中空值不丢弃**：`vars.get(&name)` 返回 `Some("")`（显式赋值为空）也开启 word，仅未命中（`None`）才触发丢弃。
- 行为通过题面 verbatim 端到端用例验证：`./custom_exe ${missing1}end ${existing} ${missing2}` → 子进程接收 `["...", "end", "existingsvalue"]`。

## Tech Stack

- 复用现有 Rust + std 实现，无新增依赖。
- 词法层位于 `src/parser/tokenize.rs`，本 stage 仅在 Normal 态 ` 展开两个子分支做精微调整。

## Implementation Approach

**核心策略**：tokenize 层把「未命中展开开启 token」这一副作用移除，让现有 `in_token` 标志精确反映 bash「显式 word 触发器」语义；token flush 路径已按 `in_token` 条件入栈，无需修改即可天然实现 null word removal。

**关键决策**：

1. **修改最小面**：只动 Normal 态 `${NAME}` 分支与 `$NAME` 分支中「未命中」路径的 `in_token = true`，把它从「分支末尾无条件设置」改为「命中分支内部条件设置」。其他所有分支（字面字符、引号开启、转义、命中展开、` 字面降级、双引号态展开）一律不动。
2. **`is_some()` vs 返回值非空**：以「`vars.get(&name)` 是否 `Some`」作为是否开启 token 的判据，而非「是否 push 了字符」——保证「命中且值为空串」也开启 word（对齐 bash「显式赋值的空值 ≠ 未设置」）。
3. **双引号态保持不动**：双引号开启时 `in_token` 已被 `'"'` 分支置真，未命中展开本就没设置 `in_token`，行为天然正确，零代码改动。
4. **复杂度**：仍为 O(n) 单次扫描，无新分配，无新 helper。

## Implementation Notes

- **`in_token` 语义文档化**：在 `tokenize.rs` 头注释或 `in_token` 声明处补充一句说明：「`in_token = true` 表示当前 word 已由显式片段（字面字符 / 引号 / 转义 / 命中变量展开）开启；未命中变量展开不开启 word，使 unquoted 全空 word 在 flush 时被自动丢弃（bash null word removal 语义）」。
- **命中空值与未命中的区别**：在两处  `子分支中，把 `if let Some(value) = vars.get(&name) { current.push_str(value); }` + 末尾 `in_token = true;` 重构为 `if let Some(value) = vars.get(&name) { current.push_str(value); in_token = true; }`——使「命中（即便空串）开启 word，未命中不开启 word」这一语义在代码层一目了然。
- **不修改 flush 路径**：行 244–247（空白处 flush）和行 358–360（行尾 flush）均已是 `if in_token { ... }`，无需改动。
- **不动单引号 / 双引号 / 命中分支 / ` 字面降级路径**：避免不必要的 blast radius，集中体现本 stage 的语义变更。
- **测试断言更新**：仅 `dollar_expansion_unquoted_miss_is_empty_token` 与新语义冲突，需要重命名 + 更新断言；其他测试用例不动。

## Architecture Design

- **数据流（不变）**：`input: &str` → `tokenize` → `Vec<String>` argv → `parse` / `parse_pipeline` → `exec`。
- **改动局部性**：只动 `tokenize.rs` 内 Normal 态 ` 展开分支两处赋值位置，整体词法状态机、错误模型、parse 层、redirect 层完全不变。

## Directory Structure

```
project-root/
├── src/parser/
│   ├── tokenize.rs   # [MODIFY] Normal 态 ${NAME} 与 $NAME 两个子分支：把「分支末尾无条件 in_token = true」改为「仅在 vars.get(&name).is_some() 时 in_token = true」；同步修订两处「未命中」注释，明确 null word removal 语义。InDoubleQuote 态展开分支保持不动；其他分支保持不动。
│   ├── mod.rs        # [MODIFY] 模块头注释追加一句：「unquoted 展开未命中且整个 word 完全为空 → word 被丢弃（bash null word removal）；含字面字符 / 引号片段 / 命中展开（即便值为空串）的 word 始终保留」。无 ParseError 变体新增。
│   └── tests.rs      # [MODIFY] 1) 重命名并更新 `dollar_expansion_unquoted_miss_is_empty_token` → `dollar_expansion_unquoted_miss_drops_word`，断言改为 `tokenize("echo $UNSET", &empty_vars()).unwrap() == vec!["echo"]`。2) 新增 6 条覆盖 null word removal 边界的用例（见下）。3) `dollar_expansion_double_quoted_miss_keeps_token` / `brace_expansion_double_quoted_miss_keeps_token` / `brace_expansion_unquoted_miss_is_empty` 不动。
└── tests/            # [READ-ONLY 评估] plan 执行阶段先扫一眼 tests/*.rs 是否已有外部命令端到端入口；如有，补充一条覆盖题面 `${missing1}end ${existing} ${missing2}` 的集成测试；如无，跳过该步，依靠单测 + 手动 printf 验证。
```

### 新增单测清单（`src/parser/tests.rs`，命名沿用现有蛇形 + 描述风格）

1. `dollar_expansion_unquoted_miss_drops_word`（重命名自旧用例）—— `echo $UNSET` → `["echo"]`
2. `brace_expansion_unquoted_miss_alone_drops_word` —— `echo ${UNSET}` → `["echo"]`（题面 `${missing2}` 最小复现）
3. `brace_expansion_problem_statement_three_args_two_remain` —— `cmd ${M1}end ${E} ${M2}` (E=existingsvalue) → `["cmd", "end", "existingsvalue"]`（题面 verbatim）
4. `expansion_unquoted_miss_followed_by_literal_token_unaffected` —— `echo $UNSET end` → `["echo", "end"]`
5. `expansion_unquoted_consecutive_misses_drop_word` —— `echo ${U1}${U2}` → `["echo"]`
6. `expansion_unquoted_miss_with_empty_double_quote_keeps_empty_token` —— `echo ${UNSET}""` → `["echo", ""]`
7. `expansion_unquoted_miss_with_single_quote_literal_keeps_token` —— `echo ${UNSET}'a'` → `["echo", "a"]`
8. `expansion_unquoted_hit_empty_value_keeps_empty_token` —— vars=`{X: ""}`, `echo $X` → `["echo", ""]`（命中空值不触发丢弃，区分于未命中）

## Key Code Structures

两处 ` 子分支的核心结构变更（伪代码，仅示意「条件性开启 token」）：

```rust
// 修改前：
if let Some(value) = vars.get(&name) {
    current.push_str(value);
}
in_token = true;   // <-- 无条件开启

// 修改后：
if let Some(value) = vars.get(&name) {
    current.push_str(value);
    in_token = true;   // <-- 仅命中时开启；未命中不开启，flush 时整个空 word 被丢弃
}
```