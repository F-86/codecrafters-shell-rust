---
name: stage-ep3-lcp-completion
overview: 为 completer 脚本分支（`complete -C`）的多候选场景增加 LCP（最长公共前缀）扩展：当候选共享比当前 current_word 更长的前缀时，TAB 直接补到 LCP 且不响铃；LCP 不长于当前输入时维持原"首 TAB 响铃 + 二次 TAB 列出"双 TAB 节奏。
todos:
  - id: rewire-script-lcp
    content: 改写脚本分支多候选臂：sort 后算首末项 LCP，`lcp.len() > current_word.len()` 时清状态并返回 Pair 替换 [start,pos) 为 lcp（无尾空格）；否则回落既有双 TAB 状态机
    status: completed
  - id: sync-module-doc
    content: 同步 completion.rs 顶部模块 doc 与脚本分支 doc：把"本 stage 不做 LCP 扩展"改写为"优先 LCP 扩展、否则双 TAB 状态机"
    status: completed
    dependencies:
      - rewire-script-lcp
  - id: add-lcp-edge-tests
    content: 在 mod tests 末尾追加 longest_common_prefix 边界用例：LCP 严格大于、LCP 等于 current_word、空 LCP 三组断言
    status: completed
    dependencies:
      - rewire-script-lcp
  - id: verify-build-test-lints
    content: 运行 cargo build / cargo test / read_lints，确认 108+ 测试零退化、零诊断
    status: completed
    dependencies:
      - rewire-script-lcp
      - add-lcp-edge-tests
---

## 用户需求

为 `complete -C` 注册的命令级补全脚本分支补上 **最长公共前缀（LCP）扩展** 行为，使多候选场景下，若所有候选共享比当前输入更长的公共前缀，TAB 直接把当前词补到该 LCP；否则维持既有「首次 TAB 响铃、二次 TAB 列出」节奏。

## 核心特性

- **LCP 扩展**：脚本返回 ≥2 候选且其 LCP 严格长于 `current_word` 时，把 `[pos - literal_len, pos)` 替换为 LCP（不带尾空格），不响铃、不列出，光标停在 LCP 末尾继续等用户输入。
- **响铃 + 列出兜底**：LCP 长度 ≤ `current_word` 长度（含相等）时，按现有双 TAB 状态机响铃→列出。
- **单候选保持原样**：仍走单候选臂，替换为 `<text>` + 尾空格。
- **状态纪律**：LCP 扩展触发时清空 `last_tab_script_key`（节奏作废）；其余对侧分支状态机已在分支入口清理过，无需重复。
- **测试场景验证**：`git c<TAB>`（候选 `checkout`/`cherry-pick`）→ 补成 `git che`、无响铃；`git chec<TAB>`（脚本仅返回 `checkout`）→ 补成 `git checkout `。

## 技术方案

### 修改范围

- 单文件：`src/completion.rs`
- 仅触及一处臂：`Completer::complete` 内 completer 脚本分支的 `Some(mut names) =>` 多候选臂（约 L335-L360）。
- 完全复用既有工具：`longest_common_prefix(&str, &str) -> &str`（L461）与命令名/文件名分支已验证的 LCP 模式。

### 实现策略

脚本分支多候选臂改造为「先 LCP，后双 TAB」两段式：

1. `names.sort()`（已有，候选已排序后首末项 LCP == 全集 LCP，O(n + L)）。
2. `lcp = longest_common_prefix(&names[0], names.last().unwrap())`。
3. **判定**：`if lcp.len() > ctx.current_word.len()` → LCP 扩展路径；否则进入既有双 TAB 状态机。
4. **LCP 扩展路径**：

- `self.last_tab_script_key.set(None)`（节奏作废）；
- 返回 `Ok((start, vec![Pair { display: lcp.clone(), replacement: lcp.clone() }]))`，其中 `start = pos - ctx.literal_len`，rustyline 据此替换 `[start, pos)` 为 LCP；
- **不加尾空格**（用户仍需继续输入区分候选；与命令名分支 LCP 行为一致）。

5. **双 TAB 路径**：保持当前 `last_tab_script_key` 三态机不变（首次 BEL + 记 key、二次列出 + 重画 `$ <line[..pos]>` + 清状态）。

### 关键决策与权衡

- **比较基准用 `ctx.current_word.len()` 而非 `prefix.len()`**：脚本分支替换的是「当前词」字面，`current_word` 是 tokenize 后已剥引号的当前词；LCP 的"是否比已输入更长"判定面向当前词语义而非整行 prefix，与 stage 题意 "longer than the current input" 严格对齐。tester 用例 `git c` 中 `current_word="c"` 长度 1，`lcp="che"` 长度 3，3>1 命中扩展。
- **替换起点用 `start = pos - ctx.literal_len`** 而非 `0`：与单候选臂保持一致，仅替换当前词字面，不动命令名与已输入的前置 args；规避「整行被命令名分支模式（起点 0）覆盖到 args 区」的边界陷阱。
- **不加尾空格**：题面 "completes to che and waits for more input" 明确不加空格；尾空格保留给单候选臂。
- **严格 `>` 比较**：题面 Notes "If the LCP is equal to what the user has already typed... ring the bell"——`lcp.len() == current_word.len()` 要走响铃路径，与既有命令名分支 `lcp.len() > prefix.len()` 同向严格大于。
- **状态清空时机**：仅在 LCP 扩展分支清 `last_tab_script_key`（双 TAB 节奏作废）；双 TAB 分支沿用既有 `take()` 自动清空模式，零行为漂移。

### 性能与复杂度

- LCP 计算 O(候选数 + LCP 字节长度)，候选规模 tester 用例为 2~5，开销 < 1µs；
- 不引入新 IO、不新增 fork/exec、不新增 RefCell 借用窗口；
- 与既有命令名分支 LCP 路径完全同构，CPU/内存影响等同 noop。

### 实施细节

- **不要修改单候选臂**（题面 Tests 第二条由其天然满足：脚本只返回 `checkout` 时走 L324-L334，替换为 `checkout `）；
- **不要修改 `extract_completer_context` / `run_completer_script` / `parse_completer_stdout`**：候选输入与上下文层无变化；
- **不要重复 sort**：现有 `names.sort()` 调用保留作为 LCP 算法的前置（首末项 LCP == 全集 LCP 的成立前提）；
- **doc 注释同步**：脚本分支顶部 doc（约 L29-L32）当前写着"多候选**不做 LCP 扩展**（题目本 stage 明确要求）"已与本 stage 矛盾，需改写为"多候选优先 LCP 扩展，无可扩展时走双 TAB 状态机"。

### 单测策略

在 `mod tests` 末尾追加纯函数级回归用例（与 `lcp_basic` 同风格）：

- LCP 严格大于 current_word：`longest_common_prefix("checkout", "cherry-pick")` == `"che"`，长度 3 > 1（基准断言）；
- LCP 等于 current_word（边界）：`longest_common_prefix("apply", "append")` == `"app"`，长度 3 == 3（不应触发扩展，由调用方比较断言保证）；
- 无公共前缀：`longest_common_prefix("foo", "bar")` == `""`（已有）。

由于 `Completer::complete` 涉及子进程 spawn 与 stdin/stdout，分支级集成测试由 codecrafters tester 黑盒覆盖；本地保持 `cargo test` 108 通过 + 新增 1~2 例 LCP 边界用例即可。

### 目录结构

```
project-root/
└── src/
    └── completion.rs   # [MODIFY] 仅改 Completer::complete 中 completer 脚本分支的多候选臂；
                        # 在排序后插入 LCP 计算与扩展路径，未命中时回落到既有双 TAB 状态机。
                        # 同步顶部模块 doc 中关于"本 stage 不做 LCP 扩展"的旧描述。
                        # mod tests 末尾追加 1~2 例 LCP 边界回归用例。
```