use crate::storage::{AppConfig, Storage};
use crate::text_ai::{self, TextRequest};

pub struct PreparedText {
    pub text: String,
    pub warning: Option<String>,
    pub may_insert: bool,
}

pub fn prepare(
    raw: &str,
    mode: &str,
    selection: &str,
    app_name: &str,
    config: &AppConfig,
    storage: &Storage,
) -> PreparedText {
    let local = apply_local(raw, config);
    let ai_mode = match mode {
        "translate" => Some("translate"),
        "edit" => Some("edit"),
        "ask" => Some("ask"),
        _ if config.text_mode == "polish" => Some("polish"),
        _ => None,
    };
    let Some(ai_mode) = ai_mode else {
        return PreparedText {
            text: local,
            warning: None,
            may_insert: true,
        };
    };
    if ai_mode == "edit" && selection.trim().is_empty() {
        return PreparedText {
            text: local,
            warning: Some("請先選取要編輯嘅文字，再開始語音編輯。原始轉錄已保留。".into()),
            may_insert: false,
        };
    }
    let result = (|| {
        let provider = config
            .providers
            .iter()
            .find(|p| Some(&p.id) == config.active_provider_id.as_ref())
            .ok_or("請先喺 AI 設定選擇供應商同模型")?;
        let key = storage.provider_key(&provider.id)?;
        let request = build_text_request(ai_mode, &local, selection, app_name, config);
        text_ai::process(provider, &key, &request)
    })();
    match result {
        Ok(text) => PreparedText {
            text,
            warning: None,
            may_insert: ai_mode != "ask",
        },
        Err(error) => PreparedText {
            text: local,
            warning: Some(format!(
                "AI 處理未完成：{error}。原始轉錄已保留，未自動輸入。"
            )),
            may_insert: false,
        },
    }
}

fn build_text_request(
    ai_mode: &str,
    local: &str,
    selection: &str,
    app_name: &str,
    config: &AppConfig,
) -> TextRequest {
    let rule = config
        .app_rules
        .iter()
        .find(|r| r.app_name.eq_ignore_ascii_case(app_name));
    let instructions = match rule {
        Some(rule) => format!("{}\n{}", config.custom_instructions, rule.instructions),
        None => config.custom_instructions.clone(),
    };
    TextRequest {
        mode: ai_mode.into(),
        text: local.into(),
        selected_text: (matches!(ai_mode, "edit" | "ask") && !selection.is_empty())
            .then(|| selection.into()),
        target_language: if ai_mode == "translate" {
            config.translation_language.clone()
        } else {
            config.language_variant.clone()
        },
        tone: rule
            .map(|r| r.tone.clone())
            .unwrap_or_else(|| config.tone.clone()),
        dictionary_terms: config.dictionary.iter().map(|d| d.term.clone()).collect(),
        custom_instructions: instructions,
        app_name: app_name.into(),
        web_search: ai_mode == "ask" && config.web_search_enabled,
    }
}

pub fn apply_local(raw: &str, config: &AppConfig) -> String {
    let trimmed = raw.trim();
    if let Some(snippet) = config
        .snippets
        .iter()
        .find(|s| !s.trigger.trim().is_empty() && s.trigger.trim() == trimmed)
    {
        return snippet.content.clone();
    }
    let mut text = raw.to_string();
    let mut aliases: Vec<_> = config
        .dictionary
        .iter()
        .flat_map(|entry| {
            entry
                .aliases
                .iter()
                .filter(|alias| !alias.is_empty() && *alias != &entry.term)
                .map(move |alias| (alias, &entry.term))
        })
        .collect();
    aliases.sort_by_key(|(alias, _)| std::cmp::Reverse(alias.len()));
    for (alias, term) in aliases {
        let mut result = String::new();
        let mut from = 0;
        for (index, matched) in text.match_indices(alias.as_str()) {
            let end = index + matched.len();
            let ascii_word =
                alias.is_ascii() && alias.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
            let left = text[..index].chars().next_back();
            let right = text[end..].chars().next();
            let boundary =
                |c: Option<char>| c.is_none_or(|c| !c.is_ascii_alphanumeric() && c != '_');
            if ascii_word && (!boundary(left) || !boundary(right)) {
                continue;
            }
            result.push_str(&text[from..index]);
            result.push_str(term);
            from = end;
        }
        result.push_str(&text[from..]);
        text = result;
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{AppRule, DictionaryEntry, Snippet};

    #[test]
    fn ai_requests_only_include_selection_for_edit_and_ask() {
        let config = AppConfig {
            translation_language: "English".into(),
            language_variant: "zh-Hant-HK".into(),
            tone: "natural".into(),
            custom_instructions: "Keep the style.".into(),
            web_search_enabled: true,
            dictionary: vec![DictionaryEntry {
                id: "d".into(),
                term: "Example".into(),
                aliases: vec![],
            }],
            app_rules: vec![AppRule {
                id: "r".into(),
                app_name: "Mail".into(),
                tone: "formal".into(),
                instructions: "Use short paragraphs.".into(),
            }],
            ..Default::default()
        };
        let selection = "選取文字 \n  example";
        for (mode, expected_selection) in [
            ("polish", None),
            ("translate", None),
            ("edit", Some(selection)),
            ("ask", Some(selection)),
            ("dictate", None),
        ] {
            let request = build_text_request(mode, "Spoken text", selection, "mail", &config);
            assert_eq!(
                request.selected_text.as_deref(),
                expected_selection,
                "{mode}"
            );
            assert_eq!(request.mode, mode);
            assert_eq!(request.text, "Spoken text");
            assert_eq!(
                request.target_language,
                if mode == "translate" {
                    "English"
                } else {
                    "zh-Hant-HK"
                }
            );
            assert_eq!(request.tone, "formal");
            assert_eq!(request.dictionary_terms, vec!["Example"]);
            assert_eq!(
                request.custom_instructions,
                "Keep the style.\nUse short paragraphs."
            );
            assert_eq!(request.app_name, "mail");
            assert_eq!(request.web_search, mode == "ask");
        }
    }

    #[test]
    fn ai_request_selection_preserves_existing_empty_and_whitespace_behavior() {
        let config = AppConfig::default();
        for mode in ["polish", "translate", "edit", "ask"] {
            assert_eq!(
                build_text_request(mode, "text", "", "", &config).selected_text,
                None,
                "{mode}"
            );
        }
        for mode in ["edit", "ask"] {
            assert_eq!(
                build_text_request(mode, "text", "  ", "", &config)
                    .selected_text
                    .as_deref(),
                Some("  "),
                "{mode}"
            );
        }
    }

    #[test]
    fn dictionary_preserves_words_and_unicode_and_snippets_require_exact_utterance() {
        let config = AppConfig {
            dictionary: vec![DictionaryEntry {
                id: "d".into(),
                term: "ChordMap".into(),
                aliases: vec!["chord map".into(), "CM".into()],
            }],
            snippets: vec![Snippet {
                id: "s".into(),
                trigger: "我的地址".into(),
                content: "第一行\n第二行".into(),
            }],
            ..Default::default()
        };
        assert_eq!(
            apply_local("用 chord map 同 CM；ACME 唔變。", &config),
            "用 ChordMap 同 ChordMap；ACME 唔變。"
        );
        assert_eq!(apply_local(" 我的地址 ", &config), "第一行\n第二行");
        assert_eq!(apply_local("講我的地址", &config), "講我的地址");
    }
}
