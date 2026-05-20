---
name: shell-tab-completion-longest-common-prefix
overview: 在 `ShellHelper::complete` 多候选分支前置「LCP 扩展」步骤：候选去重排序后计算最长公共前缀，若 LCP 严格长于当前 prefix 则返回单 Pair 让 rustyline 把 line 替换为 LCP（不带尾空格、清状态机）；若 LCP == prefix 则保持原双 TAB 响铃 / 列出状态机不变。
todos:
  - id: impl-lcp-completion
    content: 在 src/completion.rs 的多候选分支前置 LCP 扩展（添加 longest_common_prefix 工具函数 + 排序后取首末项 LCP + lcp.len() > prefix.len() 时返回单 Pair 触发 rustyline 行替换并清状态机；否则保留原双 TAB 路径），同步更新顶部模块 doc 为四态描述
    status: completed
  - id: verify-build-and-e2e
    content: 运行 cargo build 与 read_lints 零警告校验；编写 expect 脚本端到端验证：题面 progressive 链 `xyz_
    status: completed
    dependencies:
      - impl-lcp-completion
---

## 用户需求

扩展 Tab 自动补全，支持「最长公共前缀（LCP）」部分补全。当多候选存在时，先把当前命令名前缀扩展到所有候选共享的最长公共前缀；扩展后若仍非唯一，下一次 TAB 基于新前缀继续补；只有当最终候选唯一时才在末尾加空格。

## 核心功能

- **多候选 + LCP 可扩展**：把命令名前缀替换为 LCP，光标停在末尾，**不加尾空格**，无响铃、无列出
- **多候选 + LCP 不可扩展（LCP == 当前前缀）**：保留上一 stage 行为——首次 TAB 响铃 `\x07`，二次 TAB 按字母序双空格列出后重画 `$ <prefix>`
- **唯一候选**：直接补全为 `<name> `（带尾空格，沿用旧行为）
- **0 候选 / 参数区**：no-op
- **progressive 补全示例**：PATH 中存在 `xyz_foo / xyz_foo_bar / xyz_foo_bar_baz` 时，`xyz_<TAB>` → `xyz_foo`；键入 `_<TAB>` → `xyz_foo_bar`；键入 `_<TAB>` → `xyz_foo_bar_baz `（尾空格）

## 技术栈

- 语言：Rust（edition 2021），沿用现有 `rustyline = "14"` 的 Helper/Completer 体系
- 文件改动范围：仅 `src/completion.rs`（`main.rs` 与 `builtins.rs` 不动）

## 实现策略

在 `Completer::complete` 现有「多候选 (`≥2`)」分支头部**前置插入** LCP 计算与分流，把状态机由「三态」升级为「四态」：

| 候选数 | LCP 与 prefix 关系 | 行为 |
| --- | --- | --- |
| 0 | — | 清状态，no-op |
| 1 | — | 替换为 `<name> `（含尾空格），清状态 |
| ≥2 | `lcp.len() > prefix.len()` | 返回 `(0, vec![Pair{display: lcp, replacement: lcp}])` 让 rustyline 直接把 `line[0..pos]` 替换为 LCP，光标停 LCP 末尾；清状态 |
| ≥2 | `lcp == prefix`（无法扩展） | 进入原双 TAB 状态机（首次响铃 / 二次列出 + 重画提示符） |


### 关键技术决策

- **LCP 算法**：候选已 `sort()` 字典序排序后，**首尾两项的公共前缀 == 全集 LCP**。正确性：字典序中介于首末之间的所有串，其与首末项在每个位置上的字符必定相等或居中——故全集 LCP 同时等于「首末项 LCP」。复杂度 O(n + L) 优于朴素 O(n·L)
- **rustyline 协作语义**：默认 `CompletionType::Circular` 下，`(start, vec![单 Pair])` 触发「直接替换 `line[start..pos]` 为 `replacement`、光标停 replacement 末尾、不加任何字符」。这正是 LCP 扩展所需的语义——**无需自己 print 转义序列、无需重画提示符**，复用上 stage 单候选分支已验证过的同一路径
- **`Pair.replacement` 不带空格**：仅当唯一候选时才加 `' '`；与题面 "trailing space only appears when exactly one match remains" 严格对齐
- **状态机清空时机**：候选 0 / 1 / LCP 扩展成功 / 多候选但 prefix 与上次记忆不同 → 全部清 `last_tab_prefix`。仅「多候选 + LCP == prefix + prefix 与上次记忆相同」走 take 自然取走。LCP 扩展后若新 prefix 仍无法扩展，下次 TAB 算「首次」进入响铃流程，符合 bash 行为
- **UTF-8 安全**：LCP 用 `char_indices` 同步遍历找首个不一致的字节位置，按 char 边界截取；`prefix.len()` 与 `lcp.len()` 比较是字节长度，由于 LCP 由 char 边界截出且 prefix 来自 rustyline 保证的 char 边界 `pos`，三者皆安全

### 实现注意（Implementation Notes）

- **零额外打印**：LCP 扩展走 rustyline 的 `Pair` 替换路径，无 stdout 直写；既避免与 rustyline 内部光标状态冲突，也不需要任何 ANSI 控制序列
- **复用既有 sort**：当前 `≥2` 分支已 `names.sort()`，LCP 计算直接复用排序后结果，零额外排序成本
- **向后兼容**：上 stage 验证过的双 TAB 状态机原代码原地保留，作为「LCP 不可扩展」子分支被覆盖；所有上 stage 通过的回归用例（多候选无共同更长前缀的响铃 / 列出、单候选 + 尾空格、参数区不补、首词守卫）行为完全一致
- **doc 同步**：`completion.rs` 顶部 `//!` 模块注释里「双 TAB 状态机」段落升级为「四态分支」描述，明确 LCP 扩展的位置与状态机重置语义
- **不动 main.rs**：rustyline 配置保持默认 `Circular`，不引入额外 Config / 边际行为

## 目录结构

仅修改 1 个文件：

```
src/
└── completion.rs   # [MODIFY] 在 Completer::complete 的 ≥2 候选分支头部加 LCP 扩展前置；
                    #          新增私有 fn longest_common_prefix(&str, &str) -> &str；
                    #          顶部 //! 注释由「三态」升级为「四态」描述
```

## 关键代码契约

```rust
// 模块内私有：返回 a, b 的最长公共前缀切片（按 UTF-8 char 边界安全截取）
fn longest_common_prefix<'a>(a: &'a str, b: &str) -> &'a str;

// Completer::complete 多候选分支伪代码
match names.len() {
    0 => { self.last_tab_prefix.set(None); Ok((pos, vec![])) }
    1 => { /* 不变：补全 + 尾空格 */ }
    _ => {
        names.sort();
        let lcp = longest_common_prefix(&names[0], names.last().unwrap());
        if lcp.len() > prefix.len() {
            // LCP 扩展：清状态 + rustyline 替换 line[0..pos] 为 lcp
            self.last_tab_prefix.set(None);
            let s = lcp.to_string();
            return Ok((0, vec![Pair { display: s.clone(), replacement: s }]));
        }
        // LCP == prefix：原双 TAB 状态机（响铃 / 列出 / 重画），代码不变
    }
}
```