use crate::models::api_model::ChatMessageJson;

/// token计算函数
pub fn estimate_tokens(message: &str) -> usize {
    if message.is_empty() {
        return 0;
    }

    // 中文字符约2个token，其他字符约1个token
    let mut token_count = 0;

    for c in message.chars() {
        if c.is_whitespace() {
            // 空格不计入token
            continue;
        }

        if c.len_utf8() > 1 {
            token_count += 2;
        } else {
            token_count += 1;
        }
    }

    token_count
}

/// 计算消息列表的总token数量
pub fn calculate_total_tokens(messages: &[ChatMessageJson]) -> usize {
    if messages.is_empty() {
        return 0;
    }

    messages
        .iter()
        .map(|msg| estimate_tokens(&msg.content))
        .sum()
}

/// 上下文裁切函数
pub fn trim_context(messages: &[ChatMessageJson], max_tokens: usize) -> Vec<ChatMessageJson> {
    if messages.is_empty() {
        return Vec::new();
    }

    let total_tokens = calculate_total_tokens(messages);

    if total_tokens <= max_tokens {
        return messages.to_vec();
    }

    // 如果历史记录为空但还是超了配置项，则允许本次请求发送
    if messages.len() <= 2 {
        return messages.to_vec();
    }

    let mut trimmed_messages = Vec::with_capacity(messages.len().min(max_tokens / 10));
    let mut current_tokens = 0;

    // 反序遍历
    for message in messages.iter().rev() {
        let message_tokens = estimate_tokens(&message.content);

        // 超过限制
        if current_tokens + message_tokens > max_tokens {
            break;
        }

        trimmed_messages.insert(0, message.clone());
        current_tokens += message_tokens;
    }

    if trimmed_messages.len() < 2 {
        // 如果裁切后消息太少, 至少保留最后两条消息
        let start = if messages.len() >= 2 {
            messages.len() - 2
        } else {
            0
        };
        return messages[start..].to_vec();
    }

    trimmed_messages
}

/// 批量计算token数量
pub fn batch_estimate_tokens(messages: &[&str]) -> Vec<usize> {
    messages.iter().map(|msg| estimate_tokens(msg)).collect()
}

/// 获取token统计信息
pub fn get_token_stats(messages: &[ChatMessageJson]) -> (usize, usize, f64) {
    let total_tokens = calculate_total_tokens(messages);
    let message_count = messages.len();
    let avg_tokens = if message_count > 0 {
        total_tokens as f64 / message_count as f64
    } else {
        0.0
    };

    (total_tokens, message_count, avg_tokens)
}
