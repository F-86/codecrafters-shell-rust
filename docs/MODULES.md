# 模块清单

> 逐模块的「职责 / 公开 API / 依赖 / 关键不变量」。
> 架构总览见 [ARCHITECTURE.md](./ARCHITECTURE.md)；具体决策见 [DESIGN_DECISIONS.md](./DESIGN_DECISIONS.md)。

## 目录

- [main](#main)
- [parser](#parser)
  - [parser::tokenize](#parsertokenize)
  - [parser::parse](#parserparse)
- [builtins](#builtins)
  - [builtins::path](#builtinspath)
  - [builtins::echo](#builtinsecho)
  - [builtins::pwd](#builtinspwd)
  - [builtins::cd](#builtinscd)
  - [builtins::type_cmd](#builtinstype_cmd)
  - [builtins::complete](#builtinscomplete)
  - [builtins::jobs](#builtinsjobs)
  - [builtins::history](#builtinshistory)
  - [builtins::declare](#builtinsdeclare)
- [completion](#completion)
  - [completion::command](#completioncommand)
  - [completion::argpath](#completionargpath)
  - [completion::script](#completionscript)
  - [completion::helpers](#completionhelpers)
- [exec](#exec)
  - [exec::external](#execexternal)
  - [exec::pipeline](#execpipeline)
- [redirect](#redirect)
- [history_io](#history_io)

---

## main

| 维度 | 说明 |
|---|---|
| 职责 | REPL 主循环骨架：初始化 rustyline + helper、构造共享状态、prompt 前自动 reap、readline → parse → dispatch、退出保存 history |
| 公开 API | binary entry only |
| 依赖 | `parser` / `builtins` / `completion` / `exec` / `redirect` / `history_io` / `rustyline` |
| 关键不变量 | (1) `Rc<RefCell<_>>` 借用区间不重叠；(2) sink/err_sink 在外部命令 spawn 前 drop；(3) `save_history_to_envfile` 在 exit arm 与 Ctrl-D 路径行为一致 |

## parser

| 维度 | 说明 |
|---|---|
| 职责 | 命令行词法 + 语法解析；不依赖任何业务模块（叶节点，纯函数） |
| 公开 API | `parse_pipeline`, `parse`, `tokenize`, `ParsedCommand`, `Pipeline`, `ParseError`, `pub(crate) {is_name_start, is_name_cont}` |
| 依赖 | 仅 std |
| 关键不变量 | (1) tokenize 是 O(n) 单次线性扫描；(2) 状态机 `Normal` / `InSingleQuote` / `InDoubleQuote` 三态覆盖所有引号语义；(3) NAME 字符判定与 `builtins::declare::is_valid_identifier` 同源 |

### parser::tokenize

把输入字符串切分为扁平 token 序列，单次线性扫描完成引号、转义、`$VAR` 展开、`>` / `\|` / `&` 等操作符识别 + null word removal。
错误：`UnterminatedSingleQuote` / `UnterminatedDoubleQuote` / `TrailingBackslash` / `BadSubstitution` / `UnterminatedBraceExpansion`。

### parser::parse

在 token 序列基础上识别 6 类重定向操作符（`>` / `1>` / `>>` / `1>>` / `2>` / `2>>`），按 `\|` 切分 pipeline，组装 `Pipeline { stages, background }`。
错误：`MissingRedirectTarget` / `EmptyPipelineSegment`。

## builtins

| 维度 | 说明 |
|---|---|
| 职责 | 内建命令实现集合 + PATH 查找单一数据源 |
| 公开 API | `BUILTINS`, `run_echo`, `run_pwd`, `run_cd`, `run_type`, `run_complete`, `run_jobs`, `run_history`, `run_declare`, `Job`, `JobStatus`, `advance_job_status`, `render_done_jobs`, `retain_running_jobs`, `allocate_job_id`, `find_in_path`, `list_path_executables` |
| 依赖 | `parser`（NAME 字符判定） |
| 关键不变量 | `BUILTINS` 常量是 `type` 命中判定与新增 builtin 的单一数据源 |

### builtins::path

`find_in_path(name)` 与 `list_path_executables()`。命中条件：文件存在、是普通文件、Unix 执行位（0o111 任一）置位。与 bash 行为完全一致。

### builtins::echo

`run_echo(sink, args)`：把所有参数用单空格连接后 `writeln!` 到 sink。

### builtins::pwd

`run_pwd(sink, err_sink)`：调 `current_dir()` 写绝对路径到 sink；目录已删除 / 无权限时错误信息写 err_sink。

### builtins::cd

`run_cd(err_sink, args)`：切换 cwd；支持 `~`（HOME 环境变量），失败统一打印 `cd: <target>: No such file or directory`。

### builtins::type_cmd

`run_type(sink, err_sink, args)`：查询是 builtin / PATH 命中 / not found。文件命名为 `type_cmd` 以规避 Rust 关键字 `type`。

### builtins::complete

`run_complete(sink, err_sink, args, &mut registry)`：实现 `-C <path> <cmd>` 注册、`-p <cmd>` 查询、`-r <cmd>` 删除。registry 由 main 持有 `Rc<RefCell<HashMap>>` 跨命令存活。

### builtins::jobs

`Job` 结构 + `JobStatus` enum + 5 个函数：`run_jobs` / `advance_job_status` / `render_done_jobs` / `retain_running_jobs` / `allocate_job_id`。后台作业表唯一数据源；作业编号 = 最小可用正整数（`[1,3]→2`、`[2,3]→1`）。详见 [DESIGN_DECISIONS.md#background-reaping](./DESIGN_DECISIONS.md#background-reaping)。

### builtins::history

`run_history(sink, err_sink, args, entries)`：纯渲染函数，从 `&[String]` 输出 `{:>4}  {entry}\n` 格式。`history N` 按全局编号 `start+i+1` 输出末 N 条。文件 IO（`-r/-w/-a`）在 `history_io` 模块。

### builtins::declare

`run_declare(sink, err_sink, args, &mut vars)`：5 路分派（`NAME=VALUE` 写入 / `NAME` 空值声明 / `-p NAME` 查询命中 / `-p NAME` 未命中 / 其它静默）。
私有 helpers：
- `escape_for_double_quote` — 对 `\` `"` `$` `` ` `` 4 字符加反斜杠
- `is_valid_identifier` — `^[A-Za-z_][A-Za-z0-9_]*$` ASCII-only，复用 `parser::is_name_*`

## completion

| 维度 | 说明 |
|---|---|
| 职责 | TAB 补全：命令名 / 参数路径 / `complete -C` 脚本三套独立状态机；rustyline `Completer` trait 实现 |
| 公开 API | `ShellHelper`, `ShellHelper::new(completions: Rc<RefCell<...>>)` |
| 依赖 | `builtins::path`, `builtins::BUILTINS`, `parser::tokenize`, `rustyline` |
| 关键不变量 | 任一分支被触发即清掉对侧两套 Cell；三套 key 互斥；提示符常量 `"$ "` 与 `main.rs` 一致 |

### completion::command

命令名 TAB 状态机。候选源 = `BUILTINS` + `path_executables`（启动期缓存）。
- 0 候选：清状态，no-op
- 1 候选：`<name> `（加尾空格）
- ≥2 + LCP 可扩展：替换为 LCP（无尾空格）
- ≥2 + LCP 不可扩展：双 TAB（BEL → 二次列出）

### completion::argpath

参数位置路径补全。`split_dir_and_name` 按最后一个 `/` 切分；目录候选加尾 `/` 不加空格便于继续 TAB 进入下一层。状态 key = `(dir_part, name_prefix)` 二元组。

### completion::script

`complete -C <path> <cmd>` 注册的命令级脚本补全。`extract_completer_context` 从 `line[..pos]` 提取 `(cmd, current_word, prev_word, literal_len)` 四元组——`prev_word` 含 cmd（与 bash `complete -C` 语义对齐）。`run_completer_script` spawn 子进程并捕获 stdout，设置 `COMP_LINE` / `COMP_POINT` 环境变量。

### completion::helpers

三分支共享的纯函数：`longest_common_prefix`（首末项算法）、`extract_arg_prefix`、`split_dir_and_name`、`match_files_in_dir`、`classify_path`、`format_arg_completion`、`MatchKind` enum。无 `&self` 依赖，便于单测。

## exec

| 维度 | 说明 |
|---|---|
| 职责 | 单命令外部进程执行 + N 段管线执行 |
| 公开 API | `run_external`, `run_pipeline` |
| 依赖 | `builtins`, `parser`, `redirect` |
| 关键不变量 | (1) sink/err_sink 在 spawn 前 drop；(2) pipeline 中间 ChildStdout 必须 move 进下一段 Command；(3) 后台分支通知行走父进程 stdout 不复用 sink |

### exec::external

`run_external(cmd, line, args, parsed, sink, err_sink, jobs_table)`。前台用 `command.status()` 同步等待；后台用 `command.spawn()` 不 wait + 入 `jobs_table` + 打印 `[N] PID`。

### exec::pipeline

`run_pipeline(pipeline, jobs_table)`。`PrevOutput` 三态枚举（`None` / `Buffer(Vec<u8>)` / `ChildPipe(ChildStdout)`）驱动段间数据流。Builtin 子集 = `echo` / `pwd` / `type`；其它 builtin 按 `command not found` 处理避免污染 shell 状态。详见 [DESIGN_DECISIONS.md#pipeline-prev-output](./DESIGN_DECISIONS.md#pipeline-prev-output)。

## redirect

| 维度 | 说明 |
|---|---|
| 职责 | 重定向 sink 抽象：`Box<dyn Write>` 统一 builtin，`File` 物化外部命令 stdio |
| 公开 API | `open_sink`, `open_err_sink`, `open_file_for_redirect` |
| 依赖 | 仅 std |
| 关键不变量 | 三函数共享同一 `OpenOptions` 模式：`append=true` → `create + append`；`append=false` → `File::create`（O_WRONLY\|O_CREAT\|O_TRUNC） |

## history_io

| 维度 | 说明 |
|---|---|
| 职责 | `$HISTFILE` 启动加载 / 退出保存 + `history -r/-w/-a` 三段文件 IO；与渲染逻辑（`builtins::history`）职责分离 |
| 公开 API | `load_history_from_envfile`, `save_history_to_envfile`, `run_history_read`, `run_history_write`, `run_history_append`, `collect_history_entries`, `ShellEditor`（type alias） |
| 依赖 | `completion::ShellHelper`, `rustyline` |
| 关键不变量 | (1) 全部路径静默失败（不写 stderr、不阻断 REPL）；(2) `-a` 增量游标 `last_appended_len` 在文件**成功打开**后即推进，写失败不回滚 |
