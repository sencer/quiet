use crate::data::notification::Notification;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use tracing::{info, warn};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub ui: UiConfig,
    pub behavior: BehaviorConfig,
    pub rules: Vec<RuleConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BehaviorConfig {
    pub focus_on_action: bool,
    pub urgent_timeout_ms: u64,
    pub default_expire_timeout_ms: u64,
    pub max_notifications: usize,
    pub default_expires: bool,
}

impl Default for BehaviorConfig {
    fn default() -> Self {
        Self {
            focus_on_action: true,
            urgent_timeout_ms: 2500,
            default_expire_timeout_ms: 5000,
            max_notifications: 500,
            default_expires: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UiConfig {
    pub width: u32,
    pub max_height_ratio: f32, // fraction of screen height, e.g. 0.8
    pub font_size: f32,
    pub font_family: String,
    pub bg_color: String,          // Hex e.g. "#282a36" or "#4c4c4c"
    pub cluster_bg_color: String,  // Group bg e.g. "#4c4c6c"
    pub selected_bg_color: String, // Selection e.g. "#6495ed"
    pub urgent_bg_color: String,   // Urgent e.g. "#cd5c5c"
    pub text_color: String,
    pub urgent_text_color: String,
    pub margin_top: i32,
    pub margin_right: i32,
    pub margin_bottom: i32,
    pub margin_left: i32,
    pub show_icons: bool,
    pub icon_size: u32,
    pub icon_theme: Option<String>,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            width: 780,
            max_height_ratio: 0.85,
            font_size: 21.0,
            font_family: "Fira Mono".into(),
            bg_color: "#00000000".into(), // 100% transparent window canvas (widget.rasi)
            cluster_bg_color: "#4c4c6c".into(), // Group / active card
            selected_bg_color: "#6495ed".into(), // Selected card
            urgent_bg_color: "#cc5533".into(), // Urgent card
            text_color: "#ddccbb".into(), // Lightwhite text
            urgent_text_color: "#ffffff".into(),
            margin_top: 0,
            margin_right: 0,
            margin_bottom: 0,
            margin_left: 0,
            show_icons: true,
            icon_size: 64,
            icon_theme: Some("Papirus".into()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct RuleConfig {
    pub name: Option<String>,
    // Matching
    pub app_name: Option<String>,
    pub body_prefix: Option<String>,
    pub summary_regex: Option<String>,
    pub body_regex: Option<String>,

    // Pre-compiled regex caches (skipped during serialization)
    #[serde(skip)]
    pub compiled_summary_regex: Option<Regex>,
    #[serde(skip)]
    pub compiled_body_regex: Option<Regex>,

    // Transformations
    pub set_app_name: Option<String>,
    pub set_app_name_from_summary: Option<bool>,
    pub set_app_name_from_body_line: Option<usize>,
    pub strip_body_lines: Option<usize>,
    pub strip_body_prefix: Option<String>,
    pub set_app_icon: Option<String>,
    pub set_urgency: Option<u8>,
    pub set_expires: Option<bool>,
    pub expire_timeout_ms: Option<i32>,
    pub ignore_close: Option<bool>,
    pub group_by: Option<Vec<String>>,
}

impl Config {
    /// Loads configuration from standard paths or returns default config
    pub fn load(explicit_path: Option<&Path>) -> Self {
        if let Some(p) = explicit_path {
            if let Ok(cfg) = Self::from_file(p) {
                return cfg;
            }
        }

        let candidates = [
            dirs::config_dir().map(|d| d.join("quiet").join("config.toml")),
            dirs::config_dir().map(|d| d.join("sway").join("quiet.toml")),
            dirs::config_dir().map(|d| d.join("i3").join("quiet.toml")),
        ];

        for opt in candidates.into_iter().flatten() {
            if opt.exists() {
                match Self::from_file(&opt) {
                    Ok(cfg) => {
                        info!("Loaded user configuration from {:?}", opt);
                        return cfg;
                    }
                    Err(e) => {
                        warn!("Failed to parse config file {:?}: {}", opt, e);
                    }
                }
            }
        }

        info!("No custom config found; using default built-in configuration");
        Self::default()
    }

    pub fn from_file(path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let content = fs::read_to_string(path)?;
        let mut cfg: Config = toml::from_str(&content)?;
        cfg.compile_all_rules();
        Ok(cfg)
    }

    pub fn compile_all_rules(&mut self) {
        for rule in &mut self.rules {
            rule.compile_regexes();
        }
    }

    /// Evaluates rules in order and updates notification in-place, returning grouping keys.
    pub fn apply_and_get_keys(&self, notification: &mut Notification) -> Vec<String> {
        let mut chosen_group_by: Option<Vec<String>> = None;
        let mut has_rule_expires = false;

        for rule in &self.rules {
            if rule.matches(notification) {
                rule.apply(notification);
                if rule.set_expires.is_some() || rule.expire_timeout_ms.is_some() {
                    has_rule_expires = true;
                }
                if rule.group_by.is_some() && chosen_group_by.is_none() {
                    chosen_group_by = rule.group_by.clone();
                }
                break; // First matching rule wins (matching i3-notifier behavior)
            }
        }

        if !has_rule_expires {
            notification.expires = self.behavior.default_expires;
        }

        let fields = chosen_group_by.unwrap_or_else(|| vec!["app_name".into(), "body".into()]);
        let mut keys = Vec::with_capacity(fields.len());
        for field in fields {
            match field.as_str() {
                "app_name" => {
                    let val = if notification.app_name.is_empty() {
                        "_".to_string()
                    } else {
                        notification.app_name.clone()
                    };
                    keys.push(val);
                }
                "summary" => {
                    let val = if notification.summary.is_empty() {
                        "_".to_string()
                    } else {
                        notification.summary.clone()
                    };
                    keys.push(val);
                }
                "body" => {
                    let val = if notification.body.is_empty() {
                        "_".to_string()
                    } else {
                        notification.body.clone()
                    };
                    keys.push(val);
                }
                custom => {
                    keys.push(custom.to_string());
                }
            }
        }
        keys
    }
}

impl RuleConfig {
    pub fn compile_regexes(&mut self) {
        if let Some(ref pattern) = self.summary_regex {
            match Regex::new(pattern) {
                Ok(re) => self.compiled_summary_regex = Some(re),
                Err(e) => warn!("Invalid summary_regex {:?}: {}", pattern, e),
            }
        }
        if let Some(ref pattern) = self.body_regex {
            match Regex::new(pattern) {
                Ok(re) => self.compiled_body_regex = Some(re),
                Err(e) => warn!("Invalid body_regex {:?}: {}", pattern, e),
            }
        }
    }

    pub fn matches(&self, n: &Notification) -> bool {
        if let Some(ref expected_app) = self.app_name {
            if &n.app_name != expected_app {
                return false;
            }
        }

        if let Some(ref prefix) = self.body_prefix {
            if !n.body.starts_with(prefix) {
                return false;
            }
        }

        if let Some(ref re) = self.compiled_summary_regex {
            if !re.is_match(&n.summary) {
                return false;
            }
        } else if let Some(ref pattern) = self.summary_regex {
            if let Ok(re) = Regex::new(pattern) {
                if !re.is_match(&n.summary) {
                    return false;
                }
            }
        }

        if let Some(ref re) = self.compiled_body_regex {
            if !re.is_match(&n.body) {
                return false;
            }
        } else if let Some(ref pattern) = self.body_regex {
            if let Ok(re) = Regex::new(pattern) {
                if !re.is_match(&n.body) {
                    return false;
                }
            }
        }

        true
    }

    pub fn apply(&self, n: &mut Notification) {
        if let Some(ref prefix) = self.strip_body_prefix {
            if n.body.starts_with(prefix) {
                n.body = n.body[prefix.len()..].trim().to_string();
            }
        }

        if let Some(ref new_app) = self.set_app_name {
            n.app_name = new_app.clone();
        }

        if self.set_app_name_from_summary.unwrap_or(false) && !n.summary.is_empty() {
            n.app_name = n.summary.clone();
        }

        if let Some(line_idx) = self.set_app_name_from_body_line {
            let lines: Vec<&str> = n.body.lines().collect();
            if let Some(&target_line) = lines.get(line_idx) {
                let trimmed = target_line.trim();
                if !trimmed.is_empty() {
                    n.app_name = trimmed.to_string();
                }
            }
        }

        if let Some(strip_count) = self.strip_body_lines {
            let lines: Vec<&str> = n.body.lines().collect();
            let start = strip_count.min(lines.len());
            n.body = lines[start..].join("\n").trim().to_string();
        }

        if let Some(ref icon) = self.set_app_icon {
            n.app_icon = icon.clone();
        }

        if let Some(urgency) = self.set_urgency {
            n.urgency = urgency;
        }

        if let Some(expires) = self.set_expires {
            n.expires = expires;
            if !expires {
                n.expires_at = None;
            }
        }

        if let Some(ignore) = self.ignore_close {
            n.ignore_close = ignore;
        }

        if let Some(timeout_ms) = self.expire_timeout_ms {
            if timeout_ms > 0 {
                // If notification didn't specify an explicit timeout, apply rule timeout
                if n.expires_at.is_none() {
                    let expire_ns = n.created_at.saturating_add((timeout_ms as u64) * 1_000_000);
                    n.expires_at = Some(expire_ns);
                    n.expires = true;
                }
            } else {
                n.expires_at = None;
                n.expires = false;
            }
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        // Builtin default rules matching the user's ~/.config/sway/i3_notifier_config.py exactly!
        let rules = vec![
            // Gmail
            RuleConfig {
                name: Some("Gmail".into()),
                app_name: Some("Google Chrome".into()),
                body_prefix: Some("mail.google.com".into()),
                strip_body_prefix: Some("mail.google.com".into()),
                set_app_name: Some("Gmail".into()),
                set_app_icon: Some("gmail".into()),
                ignore_close: Some(true),
                group_by: Some(vec!["app_name".into(), "body".into()]),
                ..Default::default()
            },
            // Chat
            RuleConfig {
                name: Some("Chat".into()),
                app_name: Some("Google Chrome".into()),
                body_prefix: Some("chat.google.com".into()),
                strip_body_prefix: Some("chat.google.com".into()),
                set_app_name: Some("Chat".into()),
                set_app_icon: Some("google-chat".into()),
                group_by: Some(vec!["app_name".into(), "body".into()]),
                ..Default::default()
            },
            // Meet
            RuleConfig {
                name: Some("Meet".into()),
                app_name: Some("Google Chrome".into()),
                body_prefix: Some("meet.google.com".into()),
                strip_body_prefix: Some("meet.google.com".into()),
                set_app_name: Some("Meet".into()),
                set_app_icon: Some("meet".into()),
                group_by: Some(vec!["app_name".into(), "body".into()]),
                ..Default::default()
            },
            // WhatsApp
            RuleConfig {
                name: Some("WhatsApp".into()),
                app_name: Some("Google Chrome".into()),
                body_prefix: Some("web.whatsapp.com".into()),
                strip_body_prefix: Some("web.whatsapp.com".into()),
                set_app_name: Some("WhatsApp".into()),
                set_app_icon: Some("whatsapp".into()),
                group_by: Some(vec!["app_name".into(), "summary".into()]),
                ..Default::default()
            },
            // Twitter
            RuleConfig {
                name: Some("Twitter".into()),
                app_name: Some("Google Chrome".into()),
                body_prefix: Some("twitter.com".into()),
                strip_body_prefix: Some("twitter.com".into()),
                set_app_name: Some("Twitter".into()),
                set_app_icon: Some("twitter".into()),
                group_by: Some(vec!["app_name".into(), "body".into()]),
                ..Default::default()
            },
            // Instagram
            RuleConfig {
                name: Some("Instagram".into()),
                app_name: Some("Google Chrome".into()),
                body_prefix: Some("www.instagram.com".into()),
                strip_body_prefix: Some("www.instagram.com".into()),
                set_app_name: Some("Instagram".into()),
                set_app_icon: Some("photos".into()),
                group_by: Some(vec!["app_name".into(), "body".into()]),
                ..Default::default()
            },
            // Chrome general fallback
            RuleConfig {
                name: Some("Chrome".into()),
                app_name: Some("Google Chrome".into()),
                set_app_icon: Some("google-chrome".into()),
                set_app_name_from_body_line: Some(0),
                strip_body_lines: Some(2),
                group_by: Some(vec!["app_name".into()]),
                ..Default::default()
            },
            // notify-send: expires = true, app_name set from summary
            RuleConfig {
                name: Some("notify-send".into()),
                app_name: Some("notify-send".into()),
                set_expires: Some(true),
                expire_timeout_ms: Some(5000),
                set_app_name_from_summary: Some(true),
                set_app_icon: Some("plugin-notification".into()),
                group_by: Some(vec!["app_name".into(), "body".into()]),
                ..Default::default()
            },
        ];

        let mut cfg = Self {
            ui: UiConfig::default(),
            behavior: BehaviorConfig::default(),
            rules,
        };
        cfg.compile_all_rules();
        cfg
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gmail_rule_match_and_apply() {
        let config = Config::default();
        let mut notif = Notification::new(
            10,
            "Google Chrome".into(),
            "".into(),
            "New email from Alice".into(),
            "mail.google.com\nImportant meeting tomorrow".into(),
            vec![],
            1,
            -1,
        );

        let keys = config.apply_and_get_keys(&mut notif);
        assert_eq!(notif.app_name, "Gmail");
        assert_eq!(notif.app_icon, "gmail");
        assert_eq!(notif.body, "Important meeting tomorrow");
        assert!(notif.ignore_close);
        assert_eq!(keys, vec!["Gmail", "Important meeting tomorrow"]);
    }

    #[test]
    fn test_notify_send_rule_match_and_apply() {
        let config = Config::default();
        let mut notif = Notification::new(
            11,
            "notify-send".into(),
            "".into(),
            "System Update".into(),
            "All packages upgraded".into(),
            vec![],
            1,
            -1,
        );

        let keys = config.apply_and_get_keys(&mut notif);
        assert_eq!(notif.app_name, "System Update");
        assert_eq!(notif.app_icon, "plugin-notification");
        assert!(notif.expires);
        assert_eq!(keys, vec!["System Update", "All packages upgraded"]);
        assert!(notif.expires_at.is_some());
    }

    #[test]
    fn test_chrome_general_fallback_extraction() {
        let config = Config::default();
        let mut notif = Notification::new(
            12,
            "Google Chrome".into(),
            "".into(),
            "New PR Review".into(),
            "github.com\n\nUser requested review on PR #42".into(),
            vec![],
            1,
            -1,
        );

        let keys = config.apply_and_get_keys(&mut notif);
        assert_eq!(notif.app_name, "github.com");
        assert_eq!(notif.app_icon, "google-chrome");
        assert_eq!(notif.body, "User requested review on PR #42");
        assert_eq!(keys, vec!["github.com"]);
    }

    #[test]
    fn test_independent_strip_body_lines() {
        let rule = RuleConfig {
            app_name: Some("test-app".into()),
            strip_body_lines: Some(2),
            ..Default::default()
        };
        let mut notif = Notification::new(
            13,
            "test-app".into(),
            "".into(),
            "Title".into(),
            "Line 1\nLine 2\nActual Message Here".into(),
            vec![],
            1,
            -1,
        );
        rule.apply(&mut notif);
        assert_eq!(notif.body, "Actual Message Here");
    }
}
