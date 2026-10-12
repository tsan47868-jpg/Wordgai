use serde_json::{Value, json};

const API_URL: &str = "https://openrouter.ai/api/v1/chat/completions";
const DEFAULT_MODEL: &str = "openai/gpt-4o-mini";

pub(super) fn complete(prompt: &str) -> Result<String, String> {
    let api_key = std::env::var("OPENROUTER_API_KEY").map_err(|_| {
        "AI writing needs the OPENROUTER_API_KEY environment variable. Set it and restart WordCraft.".to_string()
    })?;
    if api_key.trim().is_empty() {
        return Err("OPENROUTER_API_KEY is empty. Set it and restart WordCraft.".into());
    }
    let model = std::env::var("OPENROUTER_MODEL")
        .ok()
        .filter(|model| !model.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_MODEL.into());
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(90))
        .build()
        .map_err(|e| format!("Couldn't create the AI request: {e}"))?;
    let response = client
        .post(API_URL)
        .bearer_auth(api_key.trim())
        .header("HTTP-Referer", "https://getartcraft.com")
        .header("X-Title", "Wordgai")
        .json(&json!({
            "model": model,
            "messages": [{"role": "user", "content": prompt}],
            "temperature": 0.7,
            "max_tokens": 8192
        }))
        .send()
        .map_err(|e| format!("OpenRouter request failed: {e}"))?
        .error_for_status()
        .map_err(|e| format!("OpenRouter returned an unsuccessful response: {e}"))?;
    let body: Value = response.json().map_err(|e| format!("Couldn't read OpenRouter's response: {e}"))?;
    body.get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"))
        .and_then(|message| message.get("content"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
        .ok_or_else(|| "OpenRouter returned no text. Check the selected model and try again.".into())
}
