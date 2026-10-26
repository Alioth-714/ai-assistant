# 13 Cline 集成

> 状态：已定稿 | 最后更新：2026-01-15 | 关联：roadmap/15-开发路线.md（阶段 0）、00-总览.md、agent.md | 对应决策：1, 22

## 定位：阶段 0 复用验证期载体

按 `roadmap/15-开发路线.md` 的**阶段 0**，开发初期**不写自研代码**，用 Cline + Ollama + `agent.md` 作为快速验证载体，跑通「角色 + 工具 + 记忆」的工作方式，产出别扭点清单，作为阶段 1 自研最小核心的需求输入。

> 反模式：**不要一上来就写完整 agent 框架**。先用成熟载体跑起来。

## 复用工具对比（选型参考）

| 工具 | 特点 | 适用 |
|------|------|------|
| Cline | VSCode 插件，工具链完整（读写/终端/浏览器） | **首选日常载体** |
| Aider | 终端原生，git 集成为核心卖点 | git 驱动开发 |
| OpenCode | 开源、支持多模型/provider | 备选 |
| Goose / OpenHands | 通用 agent 框架 | 备选 |

## Cline 配置

```
模型：Ollama
接口：http://localhost:11434
模型名：~~qwen3:8b（开发工具 / agent 编排）~~ **已移除（2026-10-22）**
（运行时只保留 7B 一个大脑；Cline 开发期载体模型方案**待定**：可用 `qwen25-7b`（注意阶段 0 实测它在 Cline 编排场景会循环/截断）或临时换云端模型，见 ops/12 §模型池与档位）
（档位映射权威见 ops/12-模型部署.md §模型池与档位）
```

## 提示词注入

把 `agent.md` 的内容作为 system prompt 注入 Cline，让 Cline 以 ai-assistant 的角色运行。

## 工具映射

Cline 内置工具与我们的工具层对应：

| Cline 工具 | 对应 |
|-----------|------|
| read_file | read_file |
| search_files | grep |
| write_to_file | write_file |
| execute_command | 扩展工具 |

## 迁移到自研（阶段 1 起）

阶段 1 起自研最小核心，Cline 阶段沉淀的能力复用：

- 工具调用逻辑复用（`src/tools/`）
- 模型调用复用（`src/models/`）
- 编排逻辑复用（`src/brain/`）
- 只替换交互层（Cline → Tauri 前端）

## 阶段 0 验证清单

- [ ] Cline + Ollama 跑通基本对话
- [ ] 注入 agent.md 后角色行为符合预期
- [ ] 工具调用正常（读写文件、grep）
- [ ] 记忆读写正常
- [ ] 产出「别扭点清单」（≥10 条，喂给阶段 1）

## 版本

- 最后更新：2026-01-15
- 对应决策：1, 22
