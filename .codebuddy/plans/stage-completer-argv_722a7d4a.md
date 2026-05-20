---
name: stage-completer-argv
overview: 将命令级补全触发面从『单 token + 尾空白』放宽到『首词已结束的全部场景』，并按 bash `complete -C` 契约把 (cmd, current_word, prev_word) 作为 argv[1..3] 传给脚本；同步调整替换区段（替换当前 token 而非纯插入）与未注册回退（一律 no-op）。
todos:
  - id: refactor-context-extractor
    content: 在 src/completion.rs 替换 extract_command_only 为 extract_completer_context：返回 (cmd, current_word, prev_word, literal_len)，复用 parser::tokenize，分离 tokenized 值与字面长度
    status: completed
  - id: extend-runner-with-argv
    content: 扩展 run_completer_script 签名加 cmd/current/prev 三参数，内部 Command::new(path).arg(cmd).arg(current).arg(prev)；其他契约（output 同步、success 检查、首行 / CRLF / 空过滤）原样保留
    status: completed
  - id: rewire-complete-dispatch
    content: 重写 Completer::complete 空白后分支：ctx 命中 + registry 命中 → 替换 [pos-literal_len, pos) 区段为 `
    status: completed
    dependencies:
      - refactor-context-extractor
      - extend-runner-with-argv
---

## 产品概述

为 codecrafters-shell-rust 的 `complete -C` 注册脚本调用机制增加完整的补全上下文传递，使脚本能根据用户当前键入的部分参数返回相关候选。

## 核心功能

- **触发面扩大**：只要光标左侧已含至少一个空白（首词已结束）且命令名在注册表命中，就调用脚本，覆盖『cmd<TAB>』『cmd p<TAB>』『cmd a1 a2<TAB>』『cmd a1 a2 <TAB>』全部形态。
- **传三参数 argv**：调用脚本时按题面契约传 argv[1]=命令名、argv[2]=当前被补全词、argv[3]=前一词（无则空串）。
- **替换『当前词』字面**：脚本 stdout 首行作为单一候选，把光标处『当前 token 的原始字面段』替换为 `<text> `（含尾空格）。当前词为空时退化为纯插入（与上 stage 等价）。
- **未注册一律 no-op**：命令未在注册表 → 不再回退到文件名补全（严格 `complete -C` 语义）。
- **脚本异常静默**：spawn 失败 / 非零退出 / stdout 空 / 非 UTF-8 → 静默 no-op，line 不变。

## 视觉效果

交互行为与 bash 一致：用户键入 `git remote set<TAB>` → 行末 `set` 被原地替换为 `set-url `（光标停在末尾空格后）；脚本失败或未注册场景下 line 字面与光标不变、无响铃、无回显污染。

## 技术栈

沿用既有项目栈：Rust 2021、rustyline 交互层、`std::process::Command` spawn 子进程。本 stage 改动全部集中在 `src/completion.rs` 单文件，无需新增 crate、无需触碰 `main.rs` 与 `parser.rs`。

## 实现策略

### 高层方案

把现有「严格 cmd+尾空白」判定 `extract_command_only` 替换为更通用的「补全上下文提取器」`extract_completer_context(line_to_pos) -> Option<CompleterContext>`，返回结构体 `{ cmd, current_word (tokenized), prev_word (tokenized), literal_len (光标处字面尾段长度) }`。`Completer::complete` 入口先调它做 dispatch，命中即查 registry → spawn 脚本（传三 argv）→ 单候选替换 `[pos - literal_len, pos)` 区间为 `<text> `。未命中或未注册：一律 no-op。

### 关键技术决策

1. **tokenized 值 vs 字面长度分离**（决策 3 隐含的关键陷阱）  
`argv[2]/argv[3]` 必须用 `parser::tokenize` 的结果（脚本看到的是 unquoted 值，与 bash COMP_WORDS 一致）；但 `start = pos - literal_len` 的 `literal_len` 必须从 `line[..pos]` 反向扫描连续非空白字节得到，否则带引号场景（`cmd 'fo<TAB>`）会算错替换起点。本 stage tester 用纯 ASCII，但分离两者代价为零，且兜底未来引号 stage。

2. **末尾空白判定 → current_word = ""**  
tokenize 不会为尾随空白补一个空 token，需在调用方显式判断 `line_to_pos.chars().next_back().map_or(false, |c| c.is_whitespace())`：若为真，`current_word = ""`、`literal_len = 0`、`prev_word = tokens.last()`（去除 cmd 后的最后实参，若 tokens 只剩 cmd 则 `prev_word = ""`）；若为假，`current_word = tokens.last()`、`prev_word = tokens[..len-1].last()`（不存在则 `""`）。

3. **`run_completer_script` 签名扩展**  
新签名 `fn run_completer_script(path: &str, cmd: &str, current_word: &str, prev_word: &str) -> Option<String>`，内部用 `Command::new(path).arg(cmd).arg(current_word).arg(prev_word).output()`。其他契约（`output()` 同步收齐 stdout、success 检查、取首行、CRLF trim、空过滤）完全沿用上 stage，0 行为漂移。

4. **registry 未命中 = no-op（决策 5）**  
原 `if let Some(cmd) = ...` 命中后还会 fall-through 到 `complete_filename_arg`。新版改为：**只要 `extract_completer_context` 命中**（即首词已结束），无论 registry 是否命中，都不再走文件名补全路径——registry 未命中直接 `return Ok((pos, Vec::new()))`；命中则走脚本流程。文件名补全分支仅在 `extract_completer_context` 返回 None 时（理论上不会发生，因为含空白即命中）保留以兜底。

**但**这会回归既有「未注册命令的参数仍可文件名补全」行为。为限制 blast radius，最稳妥的实现是：保持外层 `if prefix.chars().any(|c| c.is_whitespace())` 不变，把命令级补全分支变成「无条件接管所有空白后场景」——命中 ctx 后 registry 未命中即 `Ok((pos, Vec::new()))`；命令级 ctx 提取失败（如 tokenize 错误）才回退到 `complete_filename_arg`，与既有未闭合引号的静默路径保持一致。

5. **替换区段精确控制**  
`Pair { display: text.clone(), replacement: format!("{} ", text) }` + `Ok((pos - literal_len, vec![pair]))`。rustyline 语义：把 `line[start..pos]` 替换为 `replacement`，光标停到 `start + replacement.len()`，正好得到 `<前缀><text><空格>|`。当 `literal_len == 0` 时退化为上 stage 的「纯插入」分支，行为完全等价。

### 性能与边界

- TAB 是低频交互，spawn + tokenize 单次开销 < 10ms 量级，无优化必要。
- tokenize 失败（未闭合引号）：与 `complete_filename_arg` 的现有处理对齐——`extract_completer_context` 返回 None，外层直接 no-op，不响铃不报错。
- 字面对齐校验：若 `literal_current` 从 line 反扫得到的字节区间内出现引号字符（`'"\`），说明 tokenize 与字面长度不一致，按 no-op 退避（与现有 `complete_filename_arg` 第 145 行兜底一致）。
- prev_word 允许 = cmd 本身：题目示例 `cmd <TAB>` 时 prev="" 是因为 cmd 后无其他词；当 `cmd arg <TAB>` 时 prev="arg" 即 cmd 之后唯一实参，这与 bash COMP_PREV 一致；当 `cmd <TAB>`（只有 cmd + 尾空白）时 prev="" —— 关键判定：tokens 至少 1 项是 cmd，prev 的源是 `tokens[1..]` 区间的最后一项，区间为空则 ""。

### 实现注意（防回归）

- **状态机清理**：命令级分支被触发的所有返回路径（registry 未命中 no-op、脚本成功、脚本失败）都要 `self.last_tab_prefix.set(None); self.last_tab_arg_key.set(None);`，沿用上 stage 规则，避免命令名 / 文件名分支双 TAB 节奏污染。
- **日志**：本路径不写 stderr / 不响铃 / 不写文件，沿用上 stage 静默策略。
- **向后兼容**：题目示例 `docker <TAB>` 在新逻辑下命中 ctx={cmd:"docker", current:"", prev:"", literal_len:0}，脚本被传 `("docker","","")`、`start=pos`、`replacement="run "`——与上 stage 行为字面等价，已有 PTY 集成测试无需调整。

## 架构与目录

仅修改单文件，新增/重构 3 个私有函数 + 1 个私有结构体 + 重构测试模块。

```
project-root/
├── src/
│   └── completion.rs   # [MODIFY] 替换 extract_command_only 为 extract_completer_context；
│                       #          扩展 run_completer_script 签名加 3 个 argv 参数；
│                       #          重写 Completer::complete 的空白后分支：命令级 ctx 命中 →
│                       #          registry 查表 → spawn 传 argv → 替换『literal 当前词』段
│                       #          为 `<text> `；registry 未命中或脚本失败 → 静默 no-op；
│                       #          ctx 提取失败（tokenize 错）→ 回退既有 complete_filename_arg。
│                       #          重写 tests 模块中 cmd_only_* 6 条用例为 ctx_* 系列覆盖
│                       #          (cmd, current_word, prev_word, literal_len) 四元组断言。
```

## 关键代码结构（最小集）

```rust
/// 命令级补全上下文：从 line[..pos] 提取的脚本调用所需的全部信息。
///
/// 字段语义与 bash COMP_* 对齐：
/// - cmd: argv[1]，命令名（tokenize 后的首 token）
/// - current_word: argv[2]，当前被补全词（tokenize 值，已剥引号）
/// - prev_word: argv[3]，前一词；不存在则空串
/// - literal_len: 光标处『当前词原始字面段』的字节长度，用于计算 replacement 起点
///                start = pos - literal_len。末尾空白场景 literal_len = 0。
struct CompleterContext {
    cmd: String,
    current_word: String,
    prev_word: String,
    literal_len: usize,
}

fn extract_completer_context(line_to_pos: &str) -> Option<CompleterContext>;
fn run_completer_script(path: &str, cmd: &str, current: &str, prev: &str) -> Option<String>;
```