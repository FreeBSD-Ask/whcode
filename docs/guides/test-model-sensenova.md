# 测试模型：SenseNova 6.8 Flash Lite

**日期**：2026-10-21 | **用途**：开发期联调与验收测试 | **来源**：SenseNova 官方文档（访问日期 2026-10-21）

本文件记录用户提供的测试模型接入方式与能力边界，供 `whcode-llm` 联调、图像输入测试与
事件追踪用例使用。它**不是**产品默认供应商决策，也不构成任何可用性承诺。

## 1. 模型定位

SenseNova 6.8 Flash Lite（`sensenova-6.8-flash-lite`）是轻量级**原生多模态智能体模型**，
官方定位为面向真实复杂任务，适配数据分析与复杂信息呈现；支持文本对话、图像输入、
函数调用、流式输出与思考模式（来源：`https://www.sensenova.cn/models`、
`https://platform.sensenova.cn/docs`）。

## 2. 接入参数

| 项 | 值 |
|---|---|
| Base URL | `https://token.sensenova.cn/v1` |
| 对话端点 | `POST https://token.sensenova.cn/v1/chat/completions` |
| 模型 ID | `sensenova-6.8-flash-lite` |
| 鉴权 | `Authorization: Bearer <API_KEY>` |
| 上下文 | 256K（用户提供；官方模型总览未单列 6.8 的上下文值，标 `[待验证]`） |
| 最大输出 | 64K（官方参数 `max_tokens` 默认 65535，范围 `[1, 65536]`） |
| 思考强度 | `max`（官方 `reasoning_effort` 支持 `low`/`medium`/`high`/`max`/`none`，默认 `high`） |
| 图像输入 | 支持，见 §3 |

凭据**只**存放在本地 `script/sensenova.env`（`script/` 已被 `.gitignore` 忽略）；
禁止写入仓库文件、日志、事件载荷、测试夹具或提交信息。

## 3. 图像输入

官方文档说明：当 `content` 为内容块数组时，可用 `image_url` 类型传入图像，并与文本组合。
`image_url.url` 支持两种形式：

1. **公网 URL**：可公开访问的图片链接；
2. **Base64 Data URL**：`data:image/{format};base64,{data}`，其中 `{format}` 为 `png`、`jpeg`
   或 `webp` 等 MIME 子类型。

支持的图片格式：**JPG、JPEG、PNG、WebP**（`image/jpg`、`image/jpeg`、`image/png`、
`image/webp`）。

请求形态示例（字段名以官方文档为准）：

```json
{
  "model": "sensenova-6.8-flash-lite",
  "messages": [
    {
      "role": "user",
      "content": [
        { "type": "text", "text": "描述这张图片" },
        { "type": "image_url", "image_url": { "url": "data:image/png;base64,<BASE64>" } }
      ]
    }
  ],
  "thinking": { "type": "enabled" },
  "reasoning_effort": "max",
  "max_tokens": 65536
}
```

**限制说明**：Token Plan 官方页面未列明图片张数与总大小上限；SenseCore 原生图文对话文档
（面向多模态模型）写的是“最多 6 张图片、单请求总大小小于 45 MB”。该数值**不能**直接
当作 6.8 Flash Lite 的承诺，实际限制以接口错误响应为准，联调时按 `[待验证]` 记录。

## 4. 其他可用能力

- **流式输出**：`stream: true`，SSE 以 `data: [DONE]` 结束；`stream_options.include_usage`
  可返回用量。
- **思考模式**：默认开启；`reasoning_effort` 取 `low`/`medium`/`high`/`max`/`none`。
  官方 Token Plan 文档把 `thinking` 写成字符串，但**实测该端点要求对象**
  `{"type": "enabled"}`；传字符串会返回 400 `invalid arguments`。思考内容与最终输出共享
  `max_tokens` 配额。
- **函数调用**：标准 `tools`/`tool_choice`/`tool_call_id` 流程；工具结果以 `role: tool` 回传。
- **结构化输出**：`response_format: {"type": "json_object"}`；官方建议提示词中包含 `json`
  关键字并提供示例，且不要与思考模式同时依赖。
- **采样参数**：`temperature` 默认 1、`top_p` 默认 1、`n` 范围 1—7、`seed` 为 Beta。

## 5. 测试约定

1. 图像用例使用小图（建议 ≤ 1 MB），优先 Base64 Data URL，避免外网依赖；公网 URL 用例单独标记。
2. 每次调用记录：模型 ID、思考强度、`max_tokens`、是否流式、图像数量与字节数、耗时、用量。
3. 追踪默认 `redacted`；凭据与图片内容不得进入日志或事件正文（FR-203、D-05）。
4. 上下文 256K 与图片限制均先标 `[待验证]`，取得实测后再写入结论。
5. 额度与限流以平台账户页为准；测试失败不得以“供应商问题”跳过验收，需记录错误码与响应。

## 6. 实测记录（2026-10-21）

用 `script/probe_sensenova_schema.py`、`script/probe_thinking.py` 与
`script/test_sensenova_vision.py` 在本机验证，凭据来自 `script/sensenova.env`：

| 用例 | 结果 |
|---|---|
| 纯文本最小请求 | 200 |
| `image_url` 为对象 `{"url": data_url}` | 200，识别红色圆形 |
| `image_url` 为字符串 data URL | 200，识别红色圆形 |
| `type: image_base64` | 400 `invalid content`（该端点不接受此形态） |
| `thinking: "enabled"`（字符串） | 400 `invalid arguments` |
| `thinking: {"type":"enabled"}` | 200，返回 `reasoning_content` |
| `reasoning_effort: "max"` | 200，返回 `reasoning_content` |
| 图像理解（320×160 PNG，文字 `WHCODE-42` + 红色圆） | 输出 `文字=WHCODE-42`、`主色=红色`；用量 prompt 179 / completion 173 / reasoning 158 |

**结论**：图像输入可用，推荐 `content` 内容块 + `image_url` 对象 + `data:image/png;base64,...`；
思考参数用对象形态。图片张数与总大小上限仍未实测，保持 `[待验证]`。

## 7. 官方来源

- 模型总览与定位：`https://www.sensenova.cn/models`
- Token Plan 与模型清单、API 参数：`https://platform.sensenova.cn/docs`
- 原生图文对话（图片张数与大小限制的参考来源）：`https://console.sensecore.cn/micro/help/docs/model-as-a-service/nova/vision/ChatCompletions/`
- OpenAI 兼容模式说明：`https://www.sensecore.cn/help/docs/model-as-a-service/nova/overview/compatible-mode`
