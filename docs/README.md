# ai-assistant 文档中心

> 本地优先的个人 AI 助手，四层多 Agent 协作架构。
> 内部中文名：小明同学 | 工程名：ai-assistant

## 快速导航

| 想了解… | 读这个文件 |
|---------|-----------|
| 项目整体定位、架构总览 | [00-总览.md](./00-总览.md) |
| AI 如何按需加载文档 | [AI-INDEX.md](./AI-INDEX.md) |
| 当前开发角色的 system prompt | [agent.md](./agent.md) |
| 架构设计与分层 | [design/02-架构设计.md](./design/02-架构设计.md) |
| 权限与网络策略 | [design/03-权限模型.md](./design/03-权限模型.md) |
| 数据边界与脱敏 | [design/04-数据边界.md](./design/04-数据边界.md) |
| 失败处理与回滚 | [design/05-失败处理.md](./design/05-失败处理.md) |
| 数据迁移与身份连续性 | [design/19-数据迁移.md](./design/19-数据迁移.md) |
| 运行环境适配与硬件升级 | [design/20-运行环境适配.md](./design/20-运行环境适配.md) |
| 更新与回滚流程 | [design/21-更新与回滚.md](./design/21-更新与回滚.md) |
| 安全模型（注入/加密/审计） | [design/22-安全模型.md](./design/22-安全模型.md) |
| 多模态与扩展 | [design/23-多模态与扩展.md](./design/23-多模态与扩展.md) |
| 前端交互设计 | [design/24-前端交互设计.md](./design/24-前端交互设计.md) |
| 测试策略 | [design/25-测试策略.md](./design/25-测试策略.md) |
| 部署与分发（含离线包） | [design/26-部署与分发.md](./design/26-部署与分发.md) |
| 性能优化与显存调度（权威） | [design/27-性能优化.md](./design/27-性能优化.md) |
| 数据持久化与存储 | [design/28-数据持久化与存储.md](./design/28-数据持久化与存储.md) |
| 错误处理与可观测性 | [design/29-错误处理与可观测性.md](./design/29-错误处理与可观测性.md) |
| 总结与路线图 | [design/30-总结与路线图.md](./design/30-总结与路线图.md) |
| 全部架构决策记录（唯一编号权威） | [design/decisions/01-30-decisions.md](./design/decisions/01-30-decisions.md) |
| 路由层设计 | [modules/06-路由层.md](./modules/06-路由层.md) |
| 主协调者设计 | [modules/07-主协调者.md](./modules/07-主协调者.md) |
| 记忆与知识系统 | [modules/08-记忆与知识.md](./modules/08-记忆与知识.md) |
| 权限网关 | [modules/09-权限网关.md](./modules/09-权限网关.md) |
| 工具层设计 | [modules/10-工具层.md](./modules/10-工具层.md) |
| 角色切换设计 | [modules/11-角色切换.md](./modules/11-角色切换.md) |
| 模型部署与 Ollama | [ops/12-模型部署.md](./ops/12-模型部署.md) |
| Cline 集成（阶段 0 载体） | [ops/13-Cline集成.md](./ops/13-Cline集成.md) |
| 开发环境配置 | [ops/14-开发环境.md](./ops/14-开发环境.md) |
| 开发路线与阶段 | [roadmap/15-开发路线.md](./roadmap/15-开发路线.md) |
| 测试验收标准 | [roadmap/16-测试验收标准.md](./roadmap/16-测试验收标准.md) |
| 阶段 Demo 执行计划（实用版） | [roadmap/17-阶段Demo执行计划.md](./roadmap/17-阶段Demo执行计划.md) |
| 本地/云端能力边界方案 | [roadmap/18-本地云端边界方案.md](./roadmap/18-本地云端边界方案.md) |
| 阶段 4-5 实施计划（当前） | [roadmap/19-阶段4-5实施计划.md](./roadmap/19-阶段4-5实施计划.md) |

## 目录结构
docs/
├── README.md ← 本文件
├── AI-INDEX.md AI 阅读索引
├── 00-总览.md 项目总览
├── agent.md 开发角色 system prompt
├── design/ 架构与设计决策
│ ├── 01-项目定位.md
│ ├── 02-架构设计.md
│ ├── 03-权限模型.md
│ ├── 04-数据边界.md
│ ├── 05-失败处理.md
│ ├── 19-数据迁移.md
│ ├── 20-运行环境适配.md
│ ├── 21-更新与回滚.md
│ ├── 22-安全模型.md
│ ├── 23-多模态与扩展.md
│ ├── 24-前端交互设计.md
│ ├── 25-测试策略.md
│ ├── 26-部署与分发.md
│ ├── 27-性能优化.md
│ ├── 28-数据持久化与存储.md
│ ├── 29-错误处理与可观测性.md
│ ├── 30-总结与路线图.md
│ └── decisions/
│ └── 01-30-decisions.md
├── modules/ 模块详细设计
│ ├── 06-路由层.md
│ ├── 07-主协调者.md
│ ├── 08-记忆与知识.md
│ ├── 09-权限网关.md
│ ├── 10-工具层.md
│ └── 11-角色切换.md
├── ops/ 运维与部署
│ ├── 12-模型部署.md
│ ├── 13-Cline集成.md
│ └── 14-开发环境.md
└── roadmap/ 路线与验收
    ├── 15-开发路线.md
    ├── 16-测试验收标准.md
    ├── 17-阶段Demo执行计划.md
    ├── 18-本地云端边界方案.md
    └── 19-阶段4-5实施计划.md
纯文本
## 数据目录（运行时）
AI_ASSISTANT_DATA/
├── config/ JSON 配置文件
├── knowledge/ 知识库（shared/ general/ {role}/）
├── memory/ 记忆（shared/ {role}/）
├── workspace/ 任务工作区（临时，含 .locks/ 文件锁）
├── backups/ 文件级备份（批次结构）
├── cache/ 缓存（external/、role_contexts/、model_responses/）
└── logs/ 运行日志（app.log、audit.log、metrics.jsonl）
纯文本
## 核心约束（给所有读者）

1. **数据默认不出本机**；云端只接脱敏描述
2. **写操作必须走权限网关**（L2 confirm，写前备份+文件锁）
3. **数据根**：`AI_ASSISTANT_DATA` 环境变量优先；否则 debug 用 `{仓库目录}/data/`，release Windows 用 `%LOCALAPPDATA%\ai-assistant-data`，其他平台用 `$HOME/.local/share/ai-assistant/data`
4. **阶段 0-2 以本地推理为主**，不直加载 GGUF；「生成」环节本地不足时，按 `roadmap/18-本地云端边界方案.md` 走云端兜底（脱敏 + 用户确认）
5. **配置格式**：JSON（人读友好，缩进 2 空格）
6. **前端**：纯 HTML+CSS+JS（Tauri v1 承载）
7. **显存/并发调度**：见 `design/27-性能优化.md`（唯一权威）

## 版本

- 当前版本：0.1.0
- 最后更新：2026-01-15
- 决策总数：30（见 `design/decisions/01-30-decisions.md`）
- 文档总数：36 + 1 ADR（含 roadmap/17、18、19）
