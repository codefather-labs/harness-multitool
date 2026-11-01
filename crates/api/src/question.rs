//! A question to the user (`ask_user`, CON-67): what the engine checks and a frontend shows.

use serde_json::Value;

/// More would not fit a digit key.
pub const MAX_OPTIONS: usize = 9;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct QuestionOption {
    pub label: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Question {
    pub text: String,
    pub options: Vec<QuestionOption>,
}

impl Question {
    /// From the tool's input. An option may be a plain string, as small models tend to write it.
    pub fn parse(input: &Value) -> Result<Question, String> {
        let text = input["question"].as_str().map(str::trim).filter(|q| !q.is_empty()).ok_or("question is required")?.to_string();
        let mut options = Vec::new();
        for option in input["options"].as_array().into_iter().flatten() {
            let label = option.as_str().or_else(|| option["label"].as_str()).map(str::trim).filter(|l| !l.is_empty()).ok_or("each option needs a label")?;
            let description = option["description"].as_str().map(str::trim).filter(|d| !d.is_empty()).map(str::to_string);
            options.push(QuestionOption { label: label.to_string(), description });
        }
        if options.len() > MAX_OPTIONS {
            return Err(format!("at most {MAX_OPTIONS} options"));
        }
        Ok(Question { text, options })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn options_are_objects_or_strings_and_a_bad_one_says_why() {
        let q = Question::parse(&json!({"question": " Which? ", "options": ["CSV", {"label": "JSON", "description": "structured"}]})).unwrap();
        assert_eq!(q.text, "Which?");
        assert_eq!(q.options[0], QuestionOption { label: "CSV".into(), description: None });
        assert_eq!(q.options[1].description.as_deref(), Some("structured"));
        assert!(Question::parse(&json!({"question": "Open?"})).unwrap().options.is_empty());
        assert!(Question::parse(&json!({"question": " "})).is_err());
        assert!(Question::parse(&json!({"question": "x", "options": [{"description": "no label"}]})).is_err());
        assert!(Question::parse(&json!({"question": "x", "options": ["1", "2", "3", "4", "5", "6", "7", "8", "9", "10"]})).is_err());
    }
}
