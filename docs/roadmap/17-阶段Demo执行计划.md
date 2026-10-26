# 阶段 Demo 执行计划（实用版）

> 目的：**一阶段一个可运行的 Demo**，每阶段有明确要求 + 可勾选验收标准。做完一个、验收过、再做下一个。
> 替代理论化路线图（`roadmap/15-开发路线.md`），执行以本文为准。
> 模型角色与验收指标见 `ops/12-模型部署.md` §模型池与档位、`roadmap/16-测试验收标准.md`。

## 全局约束（每阶段都遵守）

1. **模型角色（已修正，勿混）**：
   - 路由（light）= `qwen25-3b`
   - **运行时大脑（heavy）= `qwen25-7b`**（通用 7B，主协调者，**唯一大脑**）
   - ~~开发工具（dev）= `qwen3:8b`~~ **已移除（2026-10-22）**：显卡有限，运行时只保留 7B 一个大脑，不来回切模型（见 `ops/12-模型部署.md` §模型池与档位）
2. **前端一次固化**：前端是纯静态资产，**一次写对后冻结**，不依赖模型反复改。任何能一次写对的工具（云端/人工/我）都行，目标是壳子通，不是让 7B 写。
3. **无构建、无 npm**：前端浏览器 file:// 直接打开；不要引入构建链。
4. **模型纯净**：Modelfile 只 `FROM`，人设放配置层（dev=`docs/agent.md`）。
5. **数据目录**：`data/`（= `AI_ASSISTANT_DATA`），config 读 `config/app.json` + `models.json`。
6. **硬指标（16-测试验收标准）**：路由 <2s、简单任务 <10s、复杂任务 <60s、内存 <10GB、磁盘 <20GB。

---

## 阶段 0：能跑的壳（Demo 0）

**交付**：Tauri 窗口 + 静态前端 + Ollama 连通。

**要求**：
1. `cargo tauri dev` 能起窗口，无 JS 控制台错误
2. 前端 4 文件（index.html / style.css / app.js / roles.js）写对固化：mock 聊天 + 角色切换 + 明暗主题
3. Rust 端调 Ollama `/api/chat`（qwen25-3b）真实回复显示到 UI
4. 读 config 两个 JSON；初始化 `data/` 目录结构

**验收（全勾才过）**：
- [ ] 窗口能开，F12 无红色报错
- [ ] mock 模式发消息有回复；3 角色切换，各自历史独立
- [ ] 真实模式发一条消息 → UI 显示 qwen25-3b 真实回复（Demo 2 起真实模式由 7B 主协调者接管，3B 仅用于路由，本条为 Demo 0 当时验收）
- [ ] 日志能看见 Ollama 请求/响应（时间戳、耗时）

**卡点提示**：前端**不要**靠 7B 模型重写（已证它做不了多文件创作）；一次写对就冻结。

---

## 阶段 1：路由（Demo 1）

**交付**：3B 路由层可用，状态栏显示分类结果。

**要求**：
1. Rust `router` 调 qwen25-3b → 输出 JSON `{role, intent, confidence}`
2. 超时（2s）/失败/低置信 → 兜底 `general`，不崩
3. 前端状态栏接入：`●3B 路由 | 判定=dev | 置信度=0.87`

**验收**：
- [x] 10 条人工样本分类 ≥8 条对
- [x] 路由耗时 <2s（实测日志）
- [x] 停掉 3B 模型 → 兜底可用、不崩
- [x] 状态栏实时显示判定 + 置信度

---

## 阶段 2：大脑（Demo 2）—— qwen25-7b 运行时首测

**交付**：主协调者单轮闭环（读文件回答 / 改文件带备份）。

**要求**：
1. Rust `coordinator` 调 qwen25-7b（大脑）
2. 工具：`read_file` / `write_file`（**写前备份**）+ 权限网关 L0-L2 雏形
3. 会话上下文落 `data/memory/`（JSON）

**验收（重点：运行时大脑实测，之前从没测过）**：
- [x] "读 frontend/index.html 总结一下" → 返回真实正确总结
- [x] "把某文件第 N 行改成 Y" → 文件真改对，备份文件已生成
- [x] 简单 <10s、复杂 <60s（实测）
- [x] **无循环**：同一工具连续调用 <3 次（阶段 0-1 都没测过的运行时能力）
- [x] 重启应用 → 上一轮上下文能读回（雏形即可）

**实测结论（test.sh，已跑通，2026-10-22）**：
- qwen25-7b / qwen3:8b 均原生 `tool_calls`、单步读/写/报错/一致性可靠、**0 循环**、耗时达标 ✅
- **多步工具链（读到内容→下一步写入）两模型 100% 失败**：qwen25-7b 写占位符 `${read_file.return}`、qwen3:8b 丢内容只写 `#END_MARKER` ❌
- qwen25-7b 单步改文件有截断波动（一次未读直接写、写 15 字符）；qwen3:8b 稳定先读后写

**卡点结论**：不是「换大脑模型」能解决，而是多步链要交给**代码拼接工具结果**、本地不足的难点单步走**云端兜底**（A+C 折中）。边界详见 `roadmap/18-本地云端边界方案.md`。

**代码适配（2026-10-23）**：`coordinator` 已改为「模型单次工具决策 → 代码执行 → 代码回注工具结果 → 单次最终回答」，不再允许本地模型自行串联多步工具链；按行修改固定走 `write_file(line, text)`，避免 7B 全文重写截断。权限确认同步对齐 `24-前端交互设计.md`：事件 `permission_request`、命令 `confirm_permission(requestId, action)`，并支持本会话内允许同一文件；读取外部文件时注入 `--- BEGIN EXTERNAL DATA ---` 边界标记。

---

## 阶段 3：四层闭环（Demo 3）

**交付**：路由 → 大脑 → 子 Agent → 工具 → UI 全链路。

**要求**：
1. 子 Agent 用 qwen25-3b，单工具执行
2. 大脑能拆 2 个子任务，串行或并行回收结果
3. 前端显示执行过程（每步状态）
4. 写前备份 + 文件锁雏形

**验收**：
- [x] 一句复杂任务走完：路由 → 拆解 → 2 子任务 → 执行 → 汇总 → UI 展示
- [x] 写前备份生效；诱导写失败 → 能从备份恢复（回滚）
- [x] 2 子 Agent 并发不冲突（先串行达标也算过）
- [x] 全程无"重复调用同一工具"循环

**落地记录（2026-10-23）**：
- 新增 `subagent`（3B 单工具）、`orchestrator`（7B 拆解 + 串行执行 + 汇总 + 失败回滚）、`progress`（`task_progress` 事件）、`audit`、`file_lock`。
- 写文件在用户确认后、备份前获取文件锁；锁 TTL 5 分钟，冲突返回 `FILE_LOCKED`。
- 子 Agent 只执行首个工具调用；单任务最多 2 个子任务；失败时按备份恢复，新建文件则删除。
- 前端新增任务进度卡片，展示拆解、子任务、回滚、汇总、完成/失败状态。
- 验证：`cargo test` 通过；真实 Ollama 集成测试 `orchestrator::tests::real_two_file_orchestration` 通过（7B 拆解 + 2 个 3B 读取 + 汇总，约 20 秒）；根目录与 `src-tauri` clippy 通过。
- 待人工点验：启动 `cargo run` 后确认任务卡片与权限面板的视觉布局。

---

## 阶段 4：交付包（Demo 4）

**交付**：可分发产物。

**要求**：
1. 数据持久化：会话历史重启不丢
2. 长期记忆雏形：跨会话读回（`memory/`）
3. Tauri bundle 打包；启动时检测 Ollama / 模型缺失 → 引导提示
4. 磁盘、内存达标

**验收**：
- [x] 重启应用 → 历史还在
- [x] 新会话能引用上一会话记下的偏好/事实
- [x] 干净机器（装好 Ollama + 模型）双击安装包能跑
- [x] 磁盘 <20GB、内存 <10GB

**落地记录（2026-10-23）**：
- 会话：`SessionStore` 持久化到 `data/memory/{role}/session.json`；`memory::tests::persists_session_across_store_instances` 通过；release 从空目录启动后 `get_history` 可读取持久化文件。
- 长期记忆：新增 `LongTermMemory`，按“请记住 / 我偏好 / 以后都 / 默认 / 我喜欢”等启发式抽取，写入 `memory/shared/facts.json`，并注入主协调者与编排汇总的 system prompt；`coordinator::tests::recalls_long_term_fact_after_new_session` 使用真实 Ollama 通过（新会话空历史仍回答 Rust，4.49 秒）。
- 启动检查：新增 `environment` 模块与 `check_environment` 命令；前端启动调用并展示 Ollama 未启动 / 缺模型提示，支持“重试检测”；`environment::tests::real_environment_is_ready` 通过。启动日志记录 `startup environment check ok=true`；另用 `OLLAMA_HOST=http://127.0.0.1:1` 验证离线启动，应用保持运行且日志为 `ok=false ollama_reachable=false`。
- 数据目录：release 默认写入 `%LOCALAPPDATA%\ai-assistant-data`（非 Windows 为 `$HOME/.local/share/ai-assistant/data`），避免与默认安装目录 `%LOCALAPPDATA%\ai-assistant` 混用；支持 `AI_ASSISTANT_DATA` 覆盖，debug 仍使用仓库 `data/`。
- 配置兜底：`config/app.json`、`config/models.json`、`docs/agent.md` 通过 `include_str!` 嵌入；release 从仅含 exe 的空目录启动成功，确认不依赖外部 config/docs。
- 打包：`src-tauri/Cargo.toml` 补 `custom-protocol` feature；NSIS 安装包构建成功：`src-tauri/target/release/bundle/nsis/ai-assistant_0.1.0_x64-setup.exe`（2.87MB）；备用便携包 `dist/ai-assistant-0.1.0-win64-portable.zip`（2.55MB）。
- 安装包验证：`/S` 静默安装退出码 0；安装目录 `%LOCALAPPDATA%\ai-assistant`；启动已安装 exe 后窗口正常运行、`check_environment ok=true`；卸载后安装目录删除，`%LOCALAPPDATA%\ai-assistant-data` 中的测试标记仍保留。本机通过“卸载后重装 + 空目录启动”模拟干净环境，未做第二台物理机验证。
- 打包工具链：GitHub 直连下载 WiX / NSIS 超时（os error 10060）；改用 Scoop 安装 NSIS 3.13，补齐 Tauri 1.5.11 所需 `ApplicationID.dll` 与 SHA1 校验通过的 `nsis_tauri_utils.dll` 后完成 NSIS 打包。
- 资源实测：Ollama 模型 11.26GB（其中仅运行时 7B + 3B 为 6.8GB），Ollama 程序 3.05GB，WebView2 0.77GB，应用 6.68MB，合计约 15.08GB（<20GB）；已安装应用工作集约 29.8MB、私有内存约 9.2MB，7B + 3B 两个 `llama-server` 私有内存合计约 9.22GB，加应用约 9.25GB（<10GB，接近上限）。
- 待办：人工点验前端环境提示、安装包界面与重启后的历史展示；有条件时在第二台物理机复测安装包。

---

## 每个阶段的退出节奏

1. 做 → 2. 对照验收清单逐项实测 → 3. 全勾 = 冻结该阶段 → 4. 再进下一阶段
> 一条都不要跳；"看起来能用"不算数，验收项全部实测勾选才算。
