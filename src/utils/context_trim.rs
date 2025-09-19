use crate::models::api_model::select_api_endpoint;
use crate::models::api_model::{ApiEndpoint, ChatMessageJson, ChatRequestJson, ChatResponseJson};
use reqwest::Client;
use std::collections::HashMap;
use tokio::task;
use uuid::Uuid;

/// token计算函数
pub fn estimate_tokens(message: &str) -> usize {
    // 更稳健的启发式：基于字节长度近似 BPE token 数量，同时对多字节字符做少量加权，并加入每条消息固定开销。
    // 经验值：平均每 token 约 4 字节（对英文）。使用 bytes_len / 4 作为基线，再加上 multi-byte 字符的轻微惩罚。
    if message.is_empty() {
        return 0;
    }

    let bytes = message.as_bytes().len();
    // 基线估算
    let mut tokens = bytes / 4;
    if tokens == 0 {
        tokens = 1;
    }

    // 对多字节字符做小幅度加权（CJK/emoji等通常占更多 token）
    let mut multi_count = 0usize;
    for b in message.as_bytes() {
        if *b >= 0x80 {
            multi_count += 1;
        }
    }
    // 每 8 个 multi-byte 字节额外加 1 token（经验值）
    tokens += multi_count / 8;

    // 每条消息增加固定开销，模拟消息元信息（role/format 等）。默认值为3，但可由配置覆盖（如果外部需要）。
    tokens + 3
}

/// 计算消息列表的总token数量
pub fn calculate_total_tokens(messages: &[ChatMessageJson]) -> usize {
    if messages.is_empty() {
        return 0;
    }

    // 缓存每条消息的估算，避免重复计算
    messages
        .iter()
        .map(|msg| estimate_tokens(&msg.content))
        .sum()
}

/// 简单摘要辅助
fn summarize_content(content: &str, max_chars: usize) -> String {
    if content.chars().count() <= max_chars {
        return content.to_string();
    }
    content.chars().take(max_chars).collect::<String>() + "…"
}

/// 使用AI端点对单个消息进行摘要
async fn summarize_message_with_ai(
    content: &str,
    max_chars: usize,
    client: &Client,
    api_endpoints: &[ApiEndpoint],
    api_headers: &HashMap<String, String>,
    summary_api_endpoints: &[ApiEndpoint],
    summary_api_max_tokens: i32,
    summary_api_temperature: f32,
    summary_api_timeout_seconds: u64,
) -> String {
    // 构造简短的请求提示
    let prompt = format!(
        "请根据以下规则处理输入文本：\n- 回答必须使用与输入文本相同的语言\n- 在保持核心含义的前提下精简文本\n- 仅返回精简结果，不要任何解释\n{}",
        content
    );

    // 优先使用摘要专用的端点，如果没有则使用通用端点
    let endpoint = if !summary_api_endpoints.is_empty() {
        select_api_endpoint(summary_api_endpoints)
    } else {
        select_api_endpoint(api_endpoints)
    };

    let endpoint = match endpoint {
        Some(ep) => ep,
        None => return summarize_content(content, max_chars),
    };

    let target_url = if endpoint.url.ends_with('/') {
        format!("{}v1/chat/completions", endpoint.url)
    } else {
        format!("{}/v1/chat/completions", endpoint.url)
    };

    // 构建请求负载
    let model = endpoint
        .model
        .clone()
        .unwrap_or_else(|| "gpt-3.5-turbo".to_string());

    let req_payload = ChatRequestJson {
        model: model.clone(),
        messages: vec![ChatMessageJson {
            role: "user".to_string(),
            content: prompt,
        }],
        temperature: summary_api_temperature,
        max_tokens: summary_api_max_tokens,
        stream: false,
        enable_thinking: None,
    };

    if let Ok(payload_json) = serde_json::to_string(&req_payload) {
        let summary_req_id: String = Uuid::new_v4().to_string().chars().take(8).collect();

        let mut request_builder = client.post(&target_url).body(payload_json.clone());
        for (k, v) in api_headers.iter() {
            request_builder = request_builder.header(k, v);
        }
        if !api_headers.contains_key("Content-Type") {
            request_builder = request_builder.header("Content-Type", "application/json");
        }
        // 便于上游/日志识别该请求为摘要
        request_builder = request_builder.header("X-Summary-Request", "true");

        // 发起请求（使用配置的超时时间）
        match tokio::time::timeout(
            std::time::Duration::from_secs(summary_api_timeout_seconds),
            request_builder.send(),
        )
        .await
        {
            Ok(Ok(resp)) => {
                if let Ok(text) = resp.text().await {
                    if let Ok(chat_resp) = serde_json::from_str::<ChatResponseJson>(&text) {
                        if !chat_resp.choices.is_empty() {
                            let s = chat_resp.choices[0].message.content.clone();
                            if !s.is_empty() {
                                return s;
                            }
                        }
                    }
                }
            }
            _ => {
                println!("[summary:{}] 请求失败/超时，回退本地摘要", summary_req_id);
            }
        }
    }

    summarize_content(content, max_chars)
}

/// 并发处理多个消息的摘要
async fn summarize_messages_concurrent(
    messages: Vec<(usize, String)>, // (index, content)
    max_chars_per_message: usize,
    client: &Client,
    api_endpoints: &[ApiEndpoint],
    api_headers: &HashMap<String, String>,
    summary_api_endpoints: &[ApiEndpoint],
    summary_api_max_tokens: i32,
    summary_api_temperature: f32,
    summary_api_timeout_seconds: u64,
    summary_mode: &str,
    summary_api_enabled: bool,
) -> Vec<(usize, String)> {
    if summary_mode != "ai"
        || !summary_api_enabled
        || api_endpoints.is_empty() && summary_api_endpoints.is_empty()
    {
        // 使用本地摘要
        return messages
            .into_iter()
            .map(|(idx, content)| (idx, summarize_content(&content, max_chars_per_message)))
            .collect();
    }

    // 创建并发任务
    let tasks: Vec<_> = messages
        .into_iter()
        .map(|(idx, content)| {
            let client = client.clone();
            let api_endpoints = api_endpoints.to_vec();
            let api_headers = api_headers.clone();
            let summary_api_endpoints = summary_api_endpoints.to_vec();

            task::spawn(async move {
                let result = summarize_message_with_ai(
                    &content,
                    max_chars_per_message,
                    &client,
                    &api_endpoints,
                    &api_headers,
                    &summary_api_endpoints,
                    summary_api_max_tokens,
                    summary_api_temperature,
                    summary_api_timeout_seconds,
                )
                .await;
                (idx, result)
            })
        })
        .collect();

    // 等待所有任务完成
    let mut results = Vec::with_capacity(tasks.len());
    for task in tasks {
        if let Ok(result) = task.await {
            results.push(result);
        }
    }

    // 按原始索引排序
    results.sort_by_key(|(idx, _)| *idx);
    results
}

/// 默认裁切：保留最后一条消息、所有 prompt 消息，以及第一轮用户对话及其对应的第一句 AI 回复。
pub fn trim_context(messages: &[ChatMessageJson], max_tokens: usize) -> Vec<ChatMessageJson> {
    if messages.is_empty() {
        return Vec::new();
    }
    let request_id: String = Uuid::new_v4().to_string().chars().take(8).collect();

    let total_tokens = calculate_total_tokens(messages);
    println!("[request_id:{}] trim_context: total_tokens={}", request_id, total_tokens);

    if total_tokens <= max_tokens {
        println!("[request_id:{}] trim_context: early return (total_tokens <= max_tokens)", request_id);
        return messages.to_vec();
    }

    // 如果历史记录为空但还是超了配置项，则允许本次请求发送
    if messages.len() <= 2 {
        println!("[request_id:{}] trim_context: history length <= 2, returning as-is", request_id);
        return messages.to_vec();
    }

    let n = messages.len();
    let mut keep = vec![false; n];
    // 始终保留最后一条
    keep[n - 1] = true;
    // 其次，保留所有 prompt 消息
    for (i, m) in messages.iter().enumerate() {
        let role = m.role.as_str();
        if role.eq_ignore_ascii_case("prompt") || role.eq_ignore_ascii_case("system") {
            keep[i] = true;
        }
    }

    // 改为按对(pair)保留：尽量保留最近的完整 user->assistant 对，避免出现孤立的单侧消息
    // 找到最后一条 user 的索引，然后与其后最近的 assistant 配对
    let mut pairs: Vec<(usize, usize)> = Vec::new();
    let mut i = 0usize;
    while i < n {
        if messages[i].role.eq_ignore_ascii_case("user") {
            // 找到对应的 assistant
            let mut j = i + 1;
            while j < n {
                if messages[j].role.eq_ignore_ascii_case("assistant") {
                    pairs.push((i, j));
                    break;
                }
                j += 1;
            }
            i = j;
        } else {
            i += 1;
        }
    }

    // 保证至少保留第一轮（如果存在）
    if !pairs.is_empty() {
        let (u, a) = pairs[0];
        keep[u] = true;
        keep[a] = true;
    }

    // 计算当前保留的 token 总数，并缓存每条消息的估算值以便复用
    let mut token_cache: Vec<usize> = Vec::with_capacity(n);
    for m in messages.iter() {
        token_cache.push(estimate_tokens(&m.content));
    }

    let mut current_tokens = 0usize;
    for i in 0..n {
        if keep[i] {
            current_tokens += token_cache[i];
        }
    }

    // 反向按消息对（若是 assistant 则尝试连同其前面的 user 一起保留）尝试添加更多消息
    let mut idx = n as isize - 1;
    while idx >= 0 {
        let i = idx as usize;
        if keep[i] {
            idx -= 1;
            continue;
        }

        let role = messages[i].role.as_str();
        if role.eq_ignore_ascii_case("assistant") {
            // 尝试连带前面的 user
            if i >= 1 && messages[i - 1].role.eq_ignore_ascii_case("user") {
                let pair_cost = token_cache[i] + token_cache[i - 1];
                if current_tokens + pair_cost <= max_tokens {
                    keep[i] = true;
                    keep[i - 1] = true;
                    current_tokens += pair_cost;
                }
                idx = idx - 2;
                continue;
            } else {
                if current_tokens + token_cache[i] <= max_tokens {
                    keep[i] = true;
                    current_tokens += token_cache[i];
                }
                idx -= 1;
                continue;
            }
        } else if role.eq_ignore_ascii_case("user") {
            // 优先保留 user 与其后 assistant 一起（如果存在）
            if i + 1 < n && messages[i + 1].role.eq_ignore_ascii_case("assistant") {
                let pair_cost = token_cache[i] + token_cache[i + 1];
                if current_tokens + pair_cost <= max_tokens {
                    keep[i] = true;
                    keep[i + 1] = true;
                    current_tokens += pair_cost;
                }
                idx -= 1;
                continue;
            } else {
                if current_tokens + token_cache[i] <= max_tokens {
                    keep[i] = true;
                    current_tokens += token_cache[i];
                }
                idx -= 1;
                continue;
            }
        } else {
            // 其他 role（如 prompt/system/function）按单条尝试
            if current_tokens + token_cache[i] <= max_tokens {
                keep[i] = true;
                current_tokens += token_cache[i];
            }
            idx -= 1;
        }
    }

    // 组装最终结果，保持原有顺序；对于被裁掉但未删除的消息，尽量做标注或摘要（这里先直接丢弃）
    let mut result = Vec::with_capacity(n);
    for i in 0..n {
        if keep[i] {
            result.push(messages[i].clone());
        }
    }

    if result.len() < 2 {
        let start = if n >= 2 { n - 2 } else { 0 };
        println!("[request_id:{}] trim_context: final_result_len=0, returning last {} messages", request_id, n - start);
        return messages[start..].to_vec();
    }

    println!("[request_id:{}] trim_context: final_result_len={}", request_id, result.len());
    result
}

/// 智能裁切：在保持所有对话的前提下，对非保留的对话进行摘要处理，以压缩内容。
pub async fn trim_context_smart(
    messages: &[ChatMessageJson],
    max_tokens: usize,
    per_message_overhead: usize,
    min_keep_pairs: usize,
    summary_aggressiveness: usize,
    summary_mode: &str,
    summary_api_enabled: bool,
    summary_api_endpoints: &[ApiEndpoint],
    summary_api_max_tokens: i32,
    summary_api_temperature: f32,
    summary_api_timeout_seconds: u64,
    client: &Client,
    api_endpoints: &[ApiEndpoint],
    api_headers: &HashMap<String, String>,
 ) -> Vec<ChatMessageJson> {
    if messages.is_empty() {
        return Vec::new();
    }
    let request_id: String = Uuid::new_v4().to_string().chars().take(8).collect();
    println!("[request_id:{}] trim_context_smart: start, n={}", request_id, messages.len());

    // 避免未使用参数的编译告警（保留签名以兼容调用方）
    let _ = min_keep_pairs;

    let n = messages.len();

    // 构建 user->assistant 对列表
    let mut pairs: Vec<(usize, usize)> = Vec::new();
    let mut i = 0usize;
    while i < n {
        if messages[i].role.eq_ignore_ascii_case("user") {
            let mut j = i + 1;
            while j < n {
                if messages[j].role.eq_ignore_ascii_case("assistant") {
                    pairs.push((i, j));
                    break;
                }
                j += 1;
            }
            i = j;
        } else {
            i += 1;
        }
    }

    // 使用摘要压缩未保留消息的内容
    let mut output = messages.to_vec();
    // 计算每条消息的 token 估算并加入 per_message_overhead
    let mut token_cache: Vec<usize> = messages
        .iter()
        .map(|m| estimate_tokens(&m.content) + per_message_overhead)
        .collect();
    let mut current_tokens = 0usize;
    // 所有消息均进行摘要
    let mut messages_to_summarize = Vec::with_capacity(n);
    let aggr = if summary_aggressiveness == 0 {
        1
    } else {
        summary_aggressiveness
    } as usize;

    for idx in 0..n {
        // 跳过 system 消息摘要，避免改写系统提示导致语言/风格漂移
        if messages[idx].role.eq_ignore_ascii_case("system") {
            continue;
        }
        let distance = n - idx; // 越早越大
        let base = if max_tokens > 0 { max_tokens / 8 } else { 32 };
        let mut approx_chars = base / aggr;
        approx_chars = std::cmp::min(
            256,
            std::cmp::max(8, approx_chars / std::cmp::max(1, distance)),
        );
        messages_to_summarize.push((idx, output[idx].content.clone(), approx_chars));
    }

    // 使用并发摘要处理所有需要摘要的消息
    if !messages_to_summarize.is_empty() {
        let summary_inputs: Vec<(usize, String)> = messages_to_summarize
            .into_iter()
            .map(|(idx, content, _)| (idx, content))
            .collect();

        // 使用固定平均字符数作为摘要长度（所有消息同一目标）
        let avg_chars = if !summary_inputs.is_empty() { 64 } else { 64 };

        let summary_results = summarize_messages_concurrent(
            summary_inputs,
            avg_chars,
            client,
            api_endpoints,
            api_headers,
            summary_api_endpoints,
            summary_api_max_tokens,
            summary_api_temperature,
            summary_api_timeout_seconds,
            summary_mode,
            summary_api_enabled,
        )
        .await;

        // 应用摘要结果
        for (idx, summarized_content) in summary_results {
            output[idx].content = summarized_content;
            token_cache[idx] = estimate_tokens(&output[idx].content) + per_message_overhead;
        }

        // 重新计算总token数
        current_tokens = token_cache.iter().sum();
        println!("[request_id:{}] trim_context_smart: 摘要后总token: {}", request_id, current_tokens);
    }

    // 如果仍然超限，按从最早到最新对所有消息做更激进压缩直至符合
    if current_tokens > max_tokens {
        for idx in 0..n {
            // 保护“最终答复”（最近一条 assistant）不被精简
            if idx + 1 == n && messages[idx].role.eq_ignore_ascii_case("assistant") {
                continue;
            }
            // 跳过 system 消息的强制精简
            if messages[idx].role.eq_ignore_ascii_case("system") {
                continue;
            }
            output[idx].content = summarize_content(&output[idx].content, 4);
            token_cache[idx] = estimate_tokens(&output[idx].content) + per_message_overhead;
            current_tokens = token_cache.iter().sum();
            if current_tokens <= max_tokens {
                break;
            }
        }
    }

    // 最后如果仍然超限（极端情况），对所有消息进行强制截短
    if current_tokens > max_tokens {
        for idx in 0..n {
            if idx + 1 == n && messages[idx].role.eq_ignore_ascii_case("assistant") {
                continue;
            }
            if messages[idx].role.eq_ignore_ascii_case("system") {
                continue;
            }
            output[idx].content = summarize_content(&output[idx].content, 2);
            token_cache[idx] = estimate_tokens(&output[idx].content) + per_message_overhead;
        }
    }

    println!(
        "智能裁切完成，最终消息数量: {}, 最终token数: {}",
        output.len(),
        calculate_total_tokens(&output)
    );
    println!("[request_id:{}] trim_context_smart: final_output_len={}", request_id, output.len());
    output
}
