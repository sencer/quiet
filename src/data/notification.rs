use serde::{Deserialize, Serialize};

use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Notification {
    pub id: u32,
    #[serde(default)]
    pub app_name: String,
    #[serde(default)]
    pub app_icon: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub actions: Vec<String>,
    #[serde(default)]
    pub urgency: u8,
    #[serde(default)]
    pub created_at: u64, // time_ns
    #[serde(default)]
    pub expires_at: Option<u64>, // time_ns
    #[serde(default)]
    pub expires: bool,
    #[serde(default)]
    pub ignore_close: bool,
    #[serde(default)]
    pub transient: bool,
    #[serde(default)]
    pub resident: bool,
    #[serde(default)]
    pub action_icons: bool,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub sound_file: Option<String>,
    #[serde(default)]
    pub suppress_sound: bool,
}

impl Notification {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: u32,
        app_name: String,
        app_icon: String,
        summary: String,
        body: String,
        actions: Vec<String>,
        urgency: u8,
        expire_timeout_ms: i32,
    ) -> Self {
        let created_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;

        let expires_at = if expire_timeout_ms > 0 {
            let expire_ns = created_at.saturating_add((expire_timeout_ms as u64) * 1_000_000);
            Some(expire_ns)
        } else {
            None
        };

        Self {
            id,
            app_name,
            app_icon,
            summary,
            body,
            actions,
            urgency,
            created_at,
            expires_at,
            expires: false,
            ignore_close: false,
            transient: false,
            resident: false,
            action_icons: false,
            category: None,
            sound_file: None,
            suppress_sound: false,
        }
    }

    /// Single line representation for i3bar / py3notifier (`Summary : Body`).
    pub fn single_line(&self) -> String {
        let clean_summary = strip_markup(&self.summary)
            .replace('\n', " ")
            .trim()
            .to_string();
        let clean_body = strip_markup(&self.body)
            .replace('\n', " ")
            .trim()
            .to_string();

        if clean_summary.is_empty() && clean_body.is_empty() {
            self.app_name.clone()
        } else if clean_body.is_empty() {
            clean_summary
        } else if clean_summary.is_empty() {
            clean_body
        } else {
            format!("{} : {}", clean_summary, clean_body)
        }
    }

    /// Returns time formatted as HH:MM in the local timezone
    pub fn time_str(&self) -> String {
        let secs = (self.created_at / 1_000_000_000) as i64;
        local_time_hh_mm(secs)
    }
}

#[repr(C)]
#[derive(Default, Debug)]
struct Tm {
    pub tm_sec: i32,
    pub tm_min: i32,
    pub tm_hour: i32,
    pub tm_mday: i32,
    pub tm_mon: i32,
    pub tm_year: i32,
    pub tm_wday: i32,
    pub tm_yday: i32,
    pub tm_isdst: i32,
    pub tm_gmtoff: i64,
    pub tm_zone: *const std::os::raw::c_char,
}

extern "C" {
    fn localtime_r(timep: *const i64, result: *mut Tm) -> *mut Tm;
}

/// Formats epoch seconds into local HH:MM with daylight savings and timezone support.
fn local_time_hh_mm(epoch_seconds: i64) -> String {
    let mut tm = Tm::default();
    let res = unsafe { localtime_r(&epoch_seconds, &mut tm) };
    if !res.is_null() {
        format!("{:02}:{:02}", tm.tm_hour, tm.tm_min)
    } else {
        let day_seconds = epoch_seconds.rem_euclid(86400);
        let hours = day_seconds / 3600;
        let minutes = (day_seconds % 3600) / 60;
        format!("{:02}:{:02}", hours, minutes)
    }
}

/// Robust HTML/Pango markup tag stripper and single-pass XML entity decoder.
pub fn strip_markup(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let chars: Vec<char> = input.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        if chars[i] == '<' {
            // Check if this looks like a valid HTML/Pango tag: e.g. <b ...>, </i>, <span ...>, <img.../>
            let mut j = i + 1;
            let is_closing = j < len && chars[j] == '/';
            if is_closing {
                j += 1;
            }
            if j < len && chars[j].is_ascii_alphabetic() {
                // Find matching '>'
                while j < len && chars[j] != '>' {
                    j += 1;
                }
                if j < len && chars[j] == '>' {
                    // Valid tag recognized, skip over it
                    i = j + 1;
                    continue;
                }
            }
            // Not a valid tag (e.g. math "x < 5"), preserve '<'
            out.push('<');
            i += 1;
        } else if chars[i] == '&' {
            // Single-pass entity decode (immune to double-unescaping, zero allocation)
            let rem = &chars[i..];
            if rem.starts_with(&['&', 'q', 'u', 'o', 't', ';']) {
                out.push('"');
                i += 6;
            } else if rem.starts_with(&['&', 'a', 'p', 'o', 's', ';']) {
                out.push('\'');
                i += 6;
            } else if rem.starts_with(&['&', 'l', 't', ';']) {
                out.push('<');
                i += 4;
            } else if rem.starts_with(&['&', 'g', 't', ';']) {
                out.push('>');
                i += 4;
            } else if rem.starts_with(&['&', 'a', 'm', 'p', ';']) {
                out.push('&');
                i += 5;
            } else if rem.starts_with(&['&', 'n', 'b', 's', 'p', ';']) {
                out.push(' ');
                i += 6;
            } else if rem.starts_with(&['&', 'h', 'e', 'l', 'l', 'i', 'p', ';']) {
                out.push('…');
                i += 8;
            } else if rem.starts_with(&['&', 'n', 'd', 'a', 's', 'h', ';']) {
                out.push('–');
                i += 7;
            } else if rem.starts_with(&['&', 'm', 'd', 'a', 's', 'h', ';']) {
                out.push('—');
                i += 7;
            } else if rem.starts_with(&['&', 'b', 'u', 'l', 'l', ';']) {
                out.push('•');
                i += 6;
            } else if rem.starts_with(&['&', 'c', 'o', 'p', 'y', ';']) {
                out.push('©');
                i += 6;
            } else if rem.starts_with(&['&', 't', 'r', 'a', 'd', 'e', ';']) {
                out.push('™');
                i += 7;
            } else if rem.starts_with(&['&', 'r', 'e', 'g', ';']) {
                out.push('®');
                i += 5;
            } else if rem.starts_with(&['&', '#', '3', '9', ';']) {
                out.push('\'');
                i += 5;
            } else if rem.starts_with(&['&', '#', '3', '4', ';']) {
                out.push('"');
                i += 5;
            } else if rem.starts_with(&['&', '#', 'x', '2', '7', ';'])
                || rem.starts_with(&['&', '#', 'X', '2', '7', ';'])
            {
                out.push('\'');
                i += 6;
            } else if rem.starts_with(&['&', '#']) {
                // Parse generic numeric entity e.g. &#8217; or &#x2019;
                let mut end = 2;
                while end < rem.len() && end < 10 && rem[end] != ';' {
                    end += 1;
                }
                if end < rem.len() && rem[end] == ';' {
                    let num_chars = &rem[2..end];
                    let parsed_char = if !num_chars.is_empty() && (num_chars[0] == 'x' || num_chars[0] == 'X') {
                        let hex_str: String = num_chars[1..].iter().collect();
                        u32::from_str_radix(&hex_str, 16).ok().and_then(char::from_u32)
                    } else {
                        let dec_str: String = num_chars.iter().collect();
                        dec_str.parse::<u32>().ok().and_then(char::from_u32)
                    };
                    if let Some(ch) = parsed_char {
                        out.push(ch);
                        i += end + 1;
                        continue;
                    }
                }
                out.push('&');
                i += 1;
            } else {
                out.push('&');
                i += 1;
            }
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_single_line() {
        let notif = Notification::new(
            1,
            "slack".into(),
            "".into(),
            "<b>Alice</b>".into(),
            "<i>Hello world</i>".into(),
            vec![],
            1,
            -1,
        );
        assert_eq!(notif.single_line(), "Alice : Hello world");
    }

    #[test]
    fn test_strip_markup() {
        assert_eq!(
            strip_markup("<span color=\"red\">Alert &amp; Warning</span>"),
            "Alert & Warning"
        );
        assert_eq!(strip_markup("x < 5 and y > 2"), "x < 5 and y > 2");
        assert_eq!(strip_markup("&amp;lt;"), "&lt;");
        assert_eq!(strip_markup("No tags here"), "No tags here");
        assert_eq!(strip_markup("Don&#39;t worry &nbsp; &quot;quote&quot;"), "Don't worry   \"quote\"");
        assert_eq!(strip_markup("Curly &#8217;apostrophe&#x2019;"), "Curly ’apostrophe’");
        assert_eq!(strip_markup("Wait&hellip; here &mdash; now &bull; done &copy;"), "Wait… here — now • done ©");
    }

    #[test]
    fn test_local_time_format() {
        let t = local_time_hh_mm(1789771415); // epoch timestamp
        assert_eq!(t.len(), 5);
        assert_eq!(&t[2..3], ":");
        let hours: u32 = t[0..2].parse().unwrap();
        let minutes: u32 = t[3..5].parse().unwrap();
        assert!(hours < 24);
        assert!(minutes < 60);
    }
}
