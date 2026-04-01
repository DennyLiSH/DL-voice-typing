/// Build the system prompt for LLM-based text correction.
///
/// The prompt is conservative: only fixes homophones and typos,
/// never rewrites or changes meaning.
pub fn build_correction_prompt() -> String {
    r#"你是一个语音转文字纠错助手。用户的文字来自语音识别，可能包含同音字错误和拼写错误。

规则：
1. 只修正明显的同音字错误（如"配森"→"Python"、"杰森"→"JSON"）
2. 只修正明显的拼写错误（如"pickage"→"package"）
3. 不要改写句子结构
4. 不要添加或删除内容
5. 中英文混合内容保持原样，不翻译
6. 如果没有错误，原样返回
7. 只输出纠错后的文本，不要解释

示例：
输入: "配森很好用" → 输出: "Python很好用"
输入: "杰森数据格式" → 输出: "JSON数据格式"
输入: "今天天气不错" → 输出: "今天天气不错"
输入: "我们用React开发" → 输出: "我们用React开发"
输入: "这个pickage需要更新" → 输出: "这个package需要更新""#
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prompt_not_empty() {
        let prompt = build_correction_prompt();
        assert!(!prompt.is_empty());
    }

    #[test]
    fn test_prompt_contains_rules() {
        let prompt = build_correction_prompt();
        assert!(prompt.contains("同音字"));
        assert!(prompt.contains("拼写"));
        assert!(prompt.contains("不要改写"));
        assert!(prompt.contains("Python"));
        assert!(prompt.contains("JSON"));
    }

    #[test]
    fn test_prompt_contains_examples() {
        let prompt = build_correction_prompt();
        assert!(prompt.contains("配森"));
        assert!(prompt.contains("杰森"));
        assert!(prompt.contains("今天天气不错"));
        assert!(prompt.contains("pickage"));
    }
}
