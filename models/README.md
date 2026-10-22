# models/ — 本地 GGUF 模型源目录

> 本目录用于放置**下载的模型文件**（GGUF），如 3B / 7B 的主模型文件。
> 对应设计：`docs/ops/12-模型部署.md`、`docs/ops/14-开发环境.md`

## 用途

- 把下载好的 GGUF（如 `qwen2.5-3b-instruct-q4_k_m.gguf`、7B 模型文件）放到这里
- 程序扫描本目录（递归深度 ≤ 5 层），识别可用模型并引导导入 Ollama

## 模型纯净原则（人设不入模型）

- 这里的 GGUF 都是**干净基底**，不带人设/系统提示
- 人设/系统提示词放**配置层**：dev 角色 = `docs/agent.md`；其他角色 = 角色 YAML 的 `prompt:` 字段
- 导入时 `Modelfile` **只写 `FROM`，不要加 `SYSTEM`**；`ollama show <模型>` 的 System 应为空
- 换模型 / 换路径 → 人设不变（人设随配置走）

## 分片 GGUF 需先合并

7B 下载下来是两段分片（`-00001-of-00002` / `-00002-of-00002`），Ollama 只认**单文件** GGUF，导入前先合并。

> ⚠️ 这批分片是 llama.cpp「延续分片」格式（第一段带完整头部、第二段只有 split 元数据）：
> - ❌ 不要用 `copy /b` 直接拼接（拼出的文件仍带分片标记，Ollama 拒绝）；
> - ⚠️ Ollama 0.40 的自动分片合并对这种格式可能报 `has 1 shards, expected 2`；
> - ✅ 用 llama.cpp 官方工具 `llama-gguf-split.exe` 合并（llama.cpp 发行包内有）。

合并命令：

```bat
cd models
"D:\APP\ggml-org\llama-b11786-bin-win-cuda-12.4-x64\llama-gguf-split.exe" --merge qwen2.5-7b-instruct-q4_k_m-00001-of-00002.gguf qwen2.5-7b-instruct-q4_k_m.gguf
```

合并成功标志：`merged from 2 split with 339 tensors`，得到 ~4.4GB 完整单文件。

> 合并后原两段分片（~4.5GB）即可删除，只保留单文件。

## 导入示例（干净 Modelfile）

在 `models/` 下建 `Modelfile`，内容只有一行 `FROM ./文件.gguf`（不加 SYSTEM），然后：

```powershell
ollama create qwen25-3b -f Modelfile   # 导入 3B（会覆盖旧的带人设版本）
# 7B：先合并分片 → 把 Modelfile 改成 FROM ./qwen2.5-7b-instruct-q4_k_m.gguf
ollama create qwen25-7b -f Modelfile
# qwen3:8b（开发工具档，Qwen3-8B Q4_K_M，原生工具调用可靠；Cline 开发用，**非运行时大脑**）
ollama create qwen3:8b -f Modelfile-qwen3-8b
ollama list                            # 确认 light/heavy/dev/embedding
```

> **模型角色**：qwen3:8b = 开发期工具（Cline 用）；运行时大脑（heavy 档）= `qwen25-7b`。两者不是一回事（见 `docs/ops/12-模型部署.md` §模型池与档位）。

## qwen3 注意（thinking 默认开）

- qwen3 默认先「思考」再回答（响应含 `thinking` 字段），会变慢、吞 token；工具调用仍正常（原生 `tool_calls`）
- 本机 Ollama 版本不支持 Modelfile 参数关闭（`enable_thinking` / `think` 均为 unknown parameter）
- 临时关闭：API 请求带 `think: false`（Cline 不会自动带，agent 用需评估）
- 模板级永久关闭：后续尝试在 `Modelfile-qwen3-8b` 覆盖 `TEMPLATE`（有风险，改后须回归测工具调用）

> 若想保留旧版带人设的 `qwen25-3b`（CBT 咨询师），导入前先 `ollama cp qwen25-3b qwen25-3b-cbt`。

## 路径可配置（默认目录不变）

- 默认路径：`{程序目录}/models/`（即本目录）
- 用户可改：环境变量 `AI_ASSISTANT_MODELS`，或 `config/models.json` 的 `models_dir` 字段，指向任意目录（如 `D:\models`）
- 改路径后本目录依然保留，导入/扫描逻辑不依赖具体路径

## 注意

- 本目录只放**源文件**（GGUF / Modelfile）；导入 Ollama 后模型存储在 `~/.ollama/models/`
- 大文件不入 git：已在工程根 `.gitignore` 中排除 `models/`
