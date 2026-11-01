//! API keys (IF-2, CON-11): per provider, from the environment variable or the configuration.
//! The OS keychain comes later. This crate is the only one that reads a key (INV-6).

use std::collections::BTreeMap;

use harness_api::{Auth, BoxFuture, PluginError, PluginResult, Secret};
use serde_json::Value;

/// Reads the environment through a function, so the crate stays free of ambient access.
pub type EnvRead = fn(&str) -> Option<String>;

pub struct ApiKeyAuth {
    env: EnvRead,
    /// `{"anthropic": {"env": "ANTHROPIC_API_KEY", "key": "..."}, ...}`.
    settings: BTreeMap<String, Value>,
}

impl ApiKeyAuth {
    pub fn new(env: EnvRead, settings: &Value) -> Self {
        let settings = settings.as_object().map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect()).unwrap_or_default();
        ApiKeyAuth { env, settings }
    }

    /// Where this provider's key comes from, for the settings; never the key.
    pub fn source(&self, provider: &str) -> String {
        let entry = self.settings.get(provider);
        let default_var = format!("{}_API_KEY", provider.to_uppercase().replace('-', "_"));
        let var = entry.and_then(|e| e["env"].as_str()).unwrap_or(&default_var);
        if (self.env)(var).is_some_and(|k| !k.is_empty()) {
            format!("from {var}")
        } else if entry.and_then(|e| e["key"].as_str()).is_some_and(|k| !k.is_empty()) {
            "saved in the configuration".to_string()
        } else if entry.and_then(|e| e["none"].as_bool()) == Some(true) {
            "none needed".to_string()
        } else {
            format!("not set (or {var})")
        }
    }

    fn lookup(&self, provider: &str) -> PluginResult<Secret> {
        let entry = self.settings.get(provider);
        let default_var = format!("{}_API_KEY", provider.to_uppercase().replace('-', "_"));
        let var = entry.and_then(|e| e["env"].as_str()).unwrap_or(&default_var);
        if let Some(key) = (self.env)(var).filter(|k| !k.is_empty()) {
            return Ok(Secret::new(key));
        }
        if let Some(key) = entry.and_then(|e| e["key"].as_str()).filter(|k| !k.is_empty()) {
            return Ok(Secret::new(key));
        }
        // A local model needs no key: its configuration says so.
        if entry.and_then(|e| e["none"].as_bool()) == Some(true) {
            return Ok(Secret::new(""));
        }
        Err(PluginError::new(format!("no API key for {provider}: set {var}, or add it to the configuration")))
    }
}

impl Auth for ApiKeyAuth {
    fn credentials(&self, provider: &str) -> BoxFuture<'_, PluginResult<Secret>> {
        Box::pin(std::future::ready(self.lookup(provider)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn env(name: &str) -> Option<String> {
        (name == "ANTHROPIC_API_KEY").then(|| "from-env".to_string())
    }

    #[test]
    fn the_environment_wins_then_the_configuration_then_an_error_naming_the_variable() {
        let auth = ApiKeyAuth::new(env, &json!({"anthropic": {"key": "from-config"}, "openai": {"key": "cfg"}, "local": {"none": true}}));
        assert_eq!(auth.lookup("anthropic").unwrap().expose(), "from-env");
        assert_eq!(auth.lookup("openai").unwrap().expose(), "cfg");
        assert_eq!(auth.lookup("local").unwrap().expose(), "");
        let err = auth.lookup("openrouter").unwrap_err().to_string();
        assert!(err.contains("OPENROUTER_API_KEY"), "{err}");
    }
}
