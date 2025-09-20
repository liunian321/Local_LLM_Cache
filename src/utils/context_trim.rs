use crate::models::api_model::select_api_endpoint;
use crate::models::api_model::{ApiEndpoint, ChatMessageJson, ChatRequestJson, ChatResponseJson};
use reqwest::Client;
use std::collections::HashMap;
use std::sync::OnceLock;
use tokio::task;
use uuid::Uuid;

// Token估算缓存
static TOKEN_CACHE: OnceLock<std::sync::Mutex<HashMap<String, usize>>> = OnceLock::new();

/// 改进的token计算函数，支持缓存和更精确的估算
pub fn estimate_tokens(message: &str) -> usize {
    if message.is_empty() {
        return 0;
    }

    // 检查缓存
    let cache = TOKEN_CACHE.get_or_init(|| std::sync::Mutex::new(HashMap::new()));
    if let Ok(cache_guard) = cache.lock() {
        if let Some(&cached_tokens) = cache_guard.get(message) {
            return cached_tokens;
        }
    }

    let tokens = estimate_tokens_internal(message);

    // 更新缓存（限制缓存大小避免内存泄漏）
    if let Ok(mut cache_guard) = cache.lock() {
        if cache_guard.len() < 10000 {
            // 限制缓存条目数
            cache_guard.insert(message.to_string(), tokens);
        }
    }

    tokens
}

/// 内部token估算实现
fn estimate_tokens_internal(message: &str) -> usize {
    let chars: Vec<char> = message.chars().collect();
    let char_count = chars.len();

    if char_count == 0 {
        return 0;
    }

    // 基于字符类型的更精确估算
    let mut tokens = 0usize;
    let mut i = 0;

    while i < char_count {
        let ch = chars[i];

        // ASCII字符：通常1个字符对应0.25-1个token
        if ch.is_ascii() {
            if ch.is_ascii_alphanumeric() {
                // 字母数字：尝试识别单词边界
                let word_start = i;
                while i < char_count && chars[i].is_ascii_alphanumeric() {
                    i += 1;
                }
                let word_len = i - word_start;
                // 英文单词平均1.3个token，短单词可能是1个token
                tokens += if word_len <= 3 {
                    1
                } else {
                    (word_len as f32 * 0.75).ceil() as usize
                };
            } else {
                // 标点符号和空格：通常1个字符1个token
                tokens += 1;
                i += 1;
            }
        }
        // CJK字符：通常1个字符对应1-2个token
        else if is_cjk_char(ch) {
            tokens += 2; // CJK字符通常占用更多token
            i += 1;
        }
        // 其他Unicode字符（包括emoji）
        else {
            tokens += if ch.len_utf8() > 2 { 3 } else { 2 };
            i += 1;
        }
    }

    // 确保最小值
    if tokens == 0 {
        tokens = 1;
    }

    // 消息固定开销（role、格式等）
    tokens + 3
}

/// 判断是否为CJK字符
fn is_cjk_char(ch: char) -> bool {
    let code = ch as u32;
    // 中文、日文、韩文的主要Unicode范围
    (0x4E00..=0x9FFF).contains(&code) ||  // CJK统一汉字
    (0x3400..=0x4DBF).contains(&code) ||  // CJK扩展A
    (0x20000..=0x2A6DF).contains(&code) || // CJK扩展B
    (0x3040..=0x309F).contains(&code) ||  // 平假名
    (0x30A0..=0x30FF).contains(&code) ||  // 片假名
    (0xAC00..=0xD7AF).contains(&code) // 韩文音节
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

/// 改进的摘要函数，按语义边界截断
fn summarize_content(content: &str, max_chars: usize) -> String {
    if content.chars().count() <= max_chars {
        return content.to_string();
    }

    // 尝试按句子边界截断
    let sentences = split_into_sentences(content);
    let mut result = String::new();
    let mut current_len = 0;

    for sentence in sentences {
        let sentence_len = sentence.chars().count();
        if current_len + sentence_len <= max_chars {
            result.push_str(sentence);
            current_len += sentence_len;
        } else {
            // 如果当前句子太长，尝试按词截断
            if result.is_empty() {
                result = truncate_by_words(sentence, max_chars);
            }
            break;
        }
    }

    // 如果结果为空或太短，回退到字符截断
    if result.is_empty() || result.chars().count() < max_chars / 2 {
        result = content
            .chars()
            .take(max_chars.saturating_sub(1))
            .collect::<String>();
    }

    if result.len() < content.len() {
        result.push('…');
    }

    result
}

/// 将文本分割为句子
fn split_into_sentences(text: &str) -> Vec<&str> {
    let mut sentences = Vec::new();
    let mut start = 0;
    let chars: Vec<char> = text.chars().collect();

    for (i, &ch) in chars.iter().enumerate() {
        // 句子结束标记
        if matches!(ch, '.' | '!' | '?' | '。' | '！' | '？' | '\n') {
            // 检查是否是真正的句子结束（避免缩写等误判）
            let is_sentence_end = if ch == '.' {
                // 简单的缩写检测
                i + 1 >= chars.len()
                    || chars[i + 1].is_whitespace()
                    || (i + 1 < chars.len() && chars[i + 1].is_uppercase())
            } else {
                true
            };

            if is_sentence_end {
                let sentence = &text[start..=text.char_indices().nth(i).unwrap().0];
                sentences.push(sentence.trim());
                start = text
                    .char_indices()
                    .nth(i + 1)
                    .map(|(pos, _)| pos)
                    .unwrap_or(text.len());
            }
        }
    }

    // 添加剩余部分
    if start < text.len() {
        sentences.push(&text[start..].trim());
    }

    sentences.into_iter().filter(|s| !s.is_empty()).collect()
}

/// 按词截断文本
fn truncate_by_words(text: &str, max_chars: usize) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    let mut result = String::new();

    for word in words {
        if result.chars().count() + word.chars().count() + 1 <= max_chars {
            if !result.is_empty() {
                result.push(' ');
            }
            result.push_str(word);
        } else {
            break;
        }
    }

    result
}

/// 计算消息重要性分数
fn calculate_message_importance(
    message: &ChatMessageJson,
    idx: usize,
    total_messages: usize,
    pairs: &[(usize, Option<usize>)],
) -> f32 {
    let mut score = 0.0f32;

    // 1. 时间新近性分数 (0.0-1.0)
    let recency_score = (total_messages - idx) as f32 / total_messages as f32;
    score += recency_score * 0.4;

    // 2. 角色重要性
    let role_score = match message.role.to_lowercase().as_str() {
        "system" | "prompt" => 1.0, // 系统消息最重要
        "user" => 0.8,              // 用户消息较重要
        "assistant" => 0.6,         // AI回复中等重要
        _ => 0.4,                   // 其他角色较低重要性
    };
    score += role_score * 0.3;

    // 3. 内容长度影响（适中长度更重要）
    let content_len = message.content.len();
    let length_score = if content_len < 50 {
        0.3 // 太短可能不重要
    } else if content_len < 500 {
        1.0 // 适中长度
    } else if content_len < 2000 {
        0.8 // 较长但仍有价值
    } else {
        0.6 // 过长可能冗余
    };
    score += length_score * 0.2;

    // 4. 对话完整性（是否为完整对话对的一部分）
    let is_in_pair = pairs.iter().any(|(user_idx, assistant_idx)| {
        *user_idx == idx || assistant_idx.map_or(false, |a_idx| a_idx == idx)
    });
    if is_in_pair {
        score += 0.1;
    }

    // 确保分数在合理范围内
    score.clamp(0.0, 1.0)
}

/// 基于重要性和内容类型计算摘要长度
fn calculate_summary_length(
    content_length: usize,
    importance_score: f32,
    aggressiveness: usize,
    role: &str,
) -> usize {
    // 基础保留比例（重要性越高保留越多）
    let base_ratio = 0.2 + (importance_score * 0.6); // 20%-80%

    // 根据激进程度调整
    let aggressiveness_factor = 1.0 - (aggressiveness as f32 * 0.1).min(0.7);
    let adjusted_ratio = base_ratio * aggressiveness_factor;

    // 角色特定调整
    let role_multiplier = match role.to_lowercase().as_str() {
        "system" | "prompt" => 1.5, // 系统消息保留更多
        "user" => 1.2,              // 用户消息稍多保留
        "assistant" => 1.0,         // AI回复正常处理
        _ => 0.8,                   // 其他角色保留较少
    };

    let target_length = (content_length as f32 * adjusted_ratio * role_multiplier) as usize;

    // 设置合理的边界
    let min_length = if content_length < 100 { 15 } else { 30 };
    let max_length = if importance_score > 0.7 { 500 } else { 300 };

    target_length.clamp(min_length, max_length)
}

/// 清理token估算缓存（用于内存管理）
pub fn clear_token_cache() {
    if let Some(cache) = TOKEN_CACHE.get() {
        if let Ok(mut cache_guard) = cache.lock() {
            cache_guard.clear();
        }
    }
}

/// 获取token缓存统计信息
pub fn get_token_cache_stats() -> (usize, usize) {
    if let Some(cache) = TOKEN_CACHE.get() {
        if let Ok(cache_guard) = cache.lock() {
            let size = cache_guard.len();
            let memory_usage = cache_guard
                .iter()
                .map(|(k, _)| k.len() + std::mem::size_of::<usize>())
                .sum::<usize>();
            return (size, memory_usage);
        }
    }
    (0, 0)
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

    // 等待所有任务完成，改进错误处理
    let mut results = Vec::with_capacity(tasks.len());
    let mut failed_count = 0;

    for task in tasks {
        match task.await {
            Ok(result) => results.push(result),
            Err(_) => {
                failed_count += 1;
                // 任务失败时记录但继续处理其他任务
            }
        }
    }

    if failed_count > 0 {
        println!(
            "[WARNING] {} AI摘要任务失败，已回退到本地摘要",
            failed_count
        );
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
    println!(
        "[request_id:{}] trim_context: total_tokens={}",
        request_id, total_tokens
    );

    if total_tokens <= max_tokens {
        println!(
            "[request_id:{}] trim_context: early return (total_tokens <= max_tokens)",
            request_id
        );
        return messages.to_vec();
    }

    // 如果历史记录为空但还是超了配置项，则允许本次请求发送
    if messages.len() <= 2 {
        println!(
            "[request_id:{}] trim_context: history length <= 2, returning as-is",
            request_id
        );
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
        println!(
            "[request_id:{}] trim_context: final_result_len=0, returning last {} messages",
            request_id,
            n - start
        );
        return messages[start..].to_vec();
    }

    println!(
        "[request_id:{}] trim_context: final_result_len={}",
        request_id,
        result.len()
    );
    result
}

/// 智能裁切：在保持对话完整性的前提下，智能选择需要摘要的消息，优化上下文压缩效果。
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
    println!(
        "[request_id:{}] trim_context_smart: start, n={}",
        request_id,
        messages.len()
    );

    let n = messages.len();
    let mut output = messages.to_vec();

    // 计算每条消息的初始 token 数
    let mut token_cache: Vec<usize> = messages
        .iter()
        .map(|m| estimate_tokens(&m.content) + per_message_overhead)
        .collect();

    let total_tokens: usize = token_cache.iter().sum();
    println!(
        "[request_id:{}] 初始总token数: {}, 目标限制: {}",
        request_id, total_tokens, max_tokens
    );

    // 如果已经在限制内，直接返回
    if total_tokens <= max_tokens {
        println!("[request_id:{}] token数已在限制内，无需裁切", request_id);
        return output;
    }

    // 构建 user->assistant 对话对列表
    let mut pairs: Vec<(usize, Option<usize>)> = Vec::new(); // (user_idx, assistant_idx)
    let mut i = 0usize;
    while i < n {
        if messages[i].role.eq_ignore_ascii_case("user") {
            let mut assistant_idx = None;
            let mut j = i + 1;
            while j < n && messages[j].role.eq_ignore_ascii_case("user") == false {
                if messages[j].role.eq_ignore_ascii_case("assistant") {
                    assistant_idx = Some(j);
                    break;
                }
                j += 1;
            }
            pairs.push((i, assistant_idx));
            i = if assistant_idx.is_some() {
                j + 1
            } else {
                i + 1
            };
        } else {
            i += 1;
        }
    }

    println!("[request_id:{}] 发现 {} 个对话对", request_id, pairs.len());

    // 标记需要保护的消息（不进行摘要）
    let mut protected = vec![false; n];

    // 1. 保护所有 system 消息
    for (i, msg) in messages.iter().enumerate() {
        if msg.role.eq_ignore_ascii_case("system") || msg.role.eq_ignore_ascii_case("prompt") {
            protected[i] = true;
        }
    }

    // 2. 保护最后几轮对话（根据 min_keep_pairs 参数）
    let keep_pairs = std::cmp::max(1, min_keep_pairs);
    let pairs_to_protect = std::cmp::min(keep_pairs, pairs.len());
    for i in (pairs.len().saturating_sub(pairs_to_protect))..pairs.len() {
        let (user_idx, assistant_idx) = pairs[i];
        protected[user_idx] = true;
        if let Some(assistant_idx) = assistant_idx {
            protected[assistant_idx] = true;
        }
    }

    // 3. 保护最后一条消息（通常是当前用户输入）
    if n > 0 {
        protected[n - 1] = true;
    }

    // 计算需要摘要的消息，使用改进的重要性评分
    let mut messages_to_summarize = Vec::new();
    let mut protected_tokens = 0usize;

    for (idx, &is_protected) in protected.iter().enumerate() {
        if is_protected {
            protected_tokens += token_cache[idx];
        } else {
            let importance_score = calculate_message_importance(&messages[idx], idx, n, &pairs);
            let content_length = messages[idx].content.len();

            // 基于重要性和内容类型计算摘要长度
            let base_length = calculate_summary_length(
                content_length,
                importance_score,
                summary_aggressiveness,
                &messages[idx].role,
            );

            messages_to_summarize.push((idx, messages[idx].content.clone(), base_length));
        }
    }

    println!(
        "[request_id:{}] 保护消息token: {}, 需摘要消息: {}",
        request_id,
        protected_tokens,
        messages_to_summarize.len()
    );

    // 执行摘要处理
    if !messages_to_summarize.is_empty() {
        let summary_inputs: Vec<(usize, String)> = messages_to_summarize
            .iter()
            .map(|(idx, content, _)| (*idx, content.clone()))
            .collect();

        // 计算平均摘要长度
        let avg_summary_length = messages_to_summarize
            .iter()
            .map(|(_, _, len)| *len)
            .sum::<usize>()
            / messages_to_summarize.len();

        let summary_results = summarize_messages_concurrent(
            summary_inputs,
            avg_summary_length,
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
            if !protected[idx] {
                output[idx].content = summarized_content;
                token_cache[idx] = estimate_tokens(&output[idx].content) + per_message_overhead;
            }
        }
    }

    // 重新计算总token数
    let current_tokens: usize = token_cache.iter().sum();
    println!(
        "[request_id:{}] 摘要后总token: {}",
        request_id, current_tokens
    );

    // 如果仍然超限，进行渐进式压缩
    if current_tokens > max_tokens {
        println!("[request_id:{}] 仍超限，进行渐进式压缩", request_id);

        // 按时间顺序（从早到晚）对未保护的消息进行更激进的压缩
        let mut remaining_tokens = current_tokens;
        let target_reduction = remaining_tokens - max_tokens;
        let mut reduced_tokens = 0usize;

        for idx in 0..n {
            if protected[idx] || reduced_tokens >= target_reduction {
                continue;
            }

            let original_tokens = token_cache[idx];
            // 根据距离当前位置的远近决定压缩程度
            let distance_factor = (n - idx) as f32 / n as f32;
            let compression_ratio = 0.1 + 0.4 * distance_factor; // 10%-50% 保留率

            let target_chars = std::cmp::max(
                8,
                (output[idx].content.len() as f32 * compression_ratio) as usize,
            );

            output[idx].content = summarize_content(&output[idx].content, target_chars);
            let new_tokens = estimate_tokens(&output[idx].content) + per_message_overhead;

            reduced_tokens += original_tokens.saturating_sub(new_tokens);
            token_cache[idx] = new_tokens;
            remaining_tokens =
                remaining_tokens.saturating_sub(original_tokens.saturating_sub(new_tokens));

            if remaining_tokens <= max_tokens {
                break;
            }
        }
    }

    // 最终检查：如果还是超限，对所有非关键消息进行极限压缩
    let final_tokens: usize = token_cache.iter().sum();
    if final_tokens > max_tokens {
        println!("[request_id:{}] 执行极限压缩", request_id);

        for idx in 0..n {
            // 保护最后一条消息和所有 system 消息
            if idx == n - 1 || messages[idx].role.eq_ignore_ascii_case("system") {
                continue;
            }

            // 极限压缩到 5-15 个字符
            let min_chars = if messages[idx].role.eq_ignore_ascii_case("assistant") {
                10
            } else {
                5
            };
            output[idx].content = summarize_content(&output[idx].content, min_chars);
            token_cache[idx] = estimate_tokens(&output[idx].content) + per_message_overhead;

            let current_total: usize = token_cache.iter().sum();
            if current_total <= max_tokens {
                break;
            }
        }
    }

    let final_total_tokens = calculate_total_tokens(&output);
    println!(
        "[request_id:{}] 智能裁切完成 - 消息数: {}, 最终token: {}, 压缩率: {:.1}%",
        request_id,
        output.len(),
        final_total_tokens,
        (1.0 - final_total_tokens as f32 / total_tokens as f32) * 100.0
    );

    output
}
