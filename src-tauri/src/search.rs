use crate::win_native;
use base64::{
    engine::general_purpose::{URL_SAFE, URL_SAFE_NO_PAD},
    Engine as _,
};
use serde::{Deserialize, Serialize};
use std::time::Duration;

const BROWSER_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36 Edg/131.0.0.0";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SearchSource {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

pub fn build_http_client(timeout_secs: u64) -> Result<reqwest::Client, String> {
    let mut builder = reqwest::Client::builder()
        .user_agent(BROWSER_UA)
        .connect_timeout(Duration::from_secs(timeout_secs.min(12)))
        .timeout(Duration::from_secs(timeout_secs));

    if let Some(proxy_url) = win_native::get_system_proxy_url() {
        if let Ok(proxy) = reqwest::Proxy::all(&proxy_url) {
            builder = builder.proxy(proxy);
        }
    }

    builder
        .build()
        .map_err(|e| format!("创建网络客户端失败: {e}"))
}

/// Free zero-API-key web search: tries Bing first (works directly in mainland China & globally),
/// and automatically falls back to DuckDuckGo HTML if Bing yields no results.
pub async fn free_web_search(query: &str, max_results: usize) -> Result<Vec<SearchSource>, String> {
    let q = query.trim();
    if q.is_empty() {
        return Ok(Vec::new());
    }
    let limit = max_results.clamp(1, 8);
    let client = build_http_client(12)?;

    if let Ok(bing_items) = search_bing(&client, q, limit).await {
        if !bing_items.is_empty() {
            return Ok(bing_items);
        }
    }

    search_duckduckgo(&client, q, limit).await
}

async fn search_bing(
    client: &reqwest::Client,
    query: &str,
    limit: usize,
) -> Result<Vec<SearchSource>, String> {
    let encoded = url_encode(query);
    let url = format!("https://cn.bing.com/search?q={encoded}&count={limit}");
    let resp = client
        .get(&url)
        .header("Accept-Language", "zh-CN,zh;q=0.9,en;q=0.8")
        .send()
        .await
        .map_err(|e| format!("Bing 搜索请求失败: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("Bing HTTP {}", resp.status()));
    }
    let html = resp.text().await.map_err(|e| e.to_string())?;
    Ok(parse_bing_html(&html, limit))
}

async fn search_duckduckgo(
    client: &reqwest::Client,
    query: &str,
    limit: usize,
) -> Result<Vec<SearchSource>, String> {
    let encoded = url_encode(query);
    let url = format!("https://html.duckduckgo.com/html/?q={encoded}");
    let resp = client
        .get(&url)
        .header("Accept-Language", "zh-CN,zh;q=0.9,en;q=0.8")
        .send()
        .await
        .map_err(|e| format!("DuckDuckGo 搜索请求失败: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("DuckDuckGo HTTP {}", resp.status()));
    }
    let html = resp.text().await.map_err(|e| e.to_string())?;
    Ok(parse_ddg_html(&html, limit))
}

/// Fetches a webpage and extracts clean readable main text (truncated to `max_chars`) for the LLM Agent.
pub async fn fetch_webpage_text(url: &str, max_chars: usize) -> Result<String, String> {
    let trimmed = url.trim();
    if !trimmed.starts_with("http://") && !trimmed.starts_with("https://") {
        return Err("仅支持 http/https 网址".to_string());
    }
    let client = build_http_client(10)?;
    let resp = client
        .get(trimmed)
        .header("Accept", "text/html,application/xhtml+xml,text/plain;q=0.9,*/*;q=0.5")
        .send()
        .await
        .map_err(|e| format!("抓取网页失败: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("网页返回 HTTP {}", resp.status()));
    }

    let bytes = resp.bytes().await.map_err(|e| e.to_string())?;
    let capped = &bytes[..bytes.len().min(320 * 1024)];
    let html = String::from_utf8_lossy(capped);
    let text = clean_html_to_readable_text(&html);
    if text.is_empty() {
        return Err("页面未提取到可读文本".to_string());
    }
    let out: String = text.chars().take(max_chars).collect();
    Ok(out)
}

fn parse_bing_html(html: &str, limit: usize) -> Vec<SearchSource> {
    let mut results = Vec::new();
    let mut cursor = html;

    while let Some(li_pos) = cursor.find("class=\"b_algo") {
        let after = &cursor[li_pos..];
        let block_end = after[10..]
            .find("class=\"b_algo")
            .map(|i| i + 10)
            .unwrap_or_else(|| after.len().min(6000));
        let block = &after[..block_end];
        cursor = &after[block_end..];

        // Find <h2> ... <a ... href="...">title</a> ... </h2>
        if let Some(h2_start) = block.find("<h2") {
            let h2_sub = &block[h2_start..];
            if let Some(h2_end) = h2_sub.find("</h2>") {
                let h2_html = &h2_sub[..h2_end];
                if let Some((raw_href, title_html)) = extract_first_anchor(h2_html) {
                    let clean_url = unwrap_bing_url(&decode_html_entities(raw_href));
                    let title = strip_tags_and_entities(title_html);
                    if clean_url.starts_with("http") && !title.is_empty() {
                        let rest = &h2_sub[h2_end + 5..];
                        let snippet = extract_paragraph_snippet(rest);
                        if !results.iter().any(|r: &SearchSource| r.url == clean_url) {
                            results.push(SearchSource {
                                title,
                                url: clean_url,
                                snippet,
                            });
                            if results.len() >= limit {
                                break;
                            }
                        }
                    }
                }
            }
        }
    }

    results
}

fn parse_ddg_html(html: &str, limit: usize) -> Vec<SearchSource> {
    let mut results = Vec::new();
    let mut cursor = html;

    while let Some(pos) = cursor.find("result__a") {
        // Walk back slightly to find `<a`
        let prefix_start = pos.saturating_sub(120);
        let window = &cursor[prefix_start..];
        let next_step = (pos - prefix_start) + 9;

        if let Some(a_rel) = window[..pos - prefix_start + 9].rfind("<a") {
            let from_a = &window[a_rel..];
            if let Some((raw_href, title_html)) = extract_first_anchor(from_a) {
                let clean_url = unwrap_ddg_url(&decode_html_entities(raw_href));
                let title = strip_tags_and_entities(title_html);

                let snippet = if let Some(sn_pos) = from_a.find("result__snippet") {
                    let sn_sub = &from_a[sn_pos..];
                    if let Some(gt) = sn_sub.find('>') {
                        let after_gt = &sn_sub[gt + 1..];
                        let end_tag = after_gt
                            .find("</a>")
                            .or_else(|| after_gt.find("</div>"))
                            .unwrap_or_else(|| after_gt.len().min(400));
                        strip_tags_and_entities(&after_gt[..end_tag])
                    } else {
                        String::new()
                    }
                } else {
                    String::new()
                };

                if clean_url.starts_with("http")
                    && !title.is_empty()
                    && !results.iter().any(|r: &SearchSource| r.url == clean_url)
                {
                    results.push(SearchSource {
                        title,
                        url: clean_url,
                        snippet,
                    });
                    if results.len() >= limit {
                        break;
                    }
                }
            }
        }

        cursor = &window[next_step.min(window.len())..];
    }

    results
}

fn extract_first_anchor(fragment: &str) -> Option<(&str, &str)> {
    let a_idx = fragment.find("<a")?;
    let after_a = &fragment[a_idx..];
    let href_pos = after_a.find("href=\"")?;
    let href_rest = &after_a[href_pos + 6..];
    let quote_end = href_rest.find('"')?;
    let href = &href_rest[..quote_end];

    let tag_close = href_rest[quote_end..].find('>')? + quote_end + 1;
    let body_rest = &href_rest[tag_close..];
    let close_a = body_rest.find("</a>")?;
    Some((href, &body_rest[..close_a]))
}

fn extract_paragraph_snippet(fragment: &str) -> String {
    if let Some(p_idx) = fragment.find("<p") {
        let after_p = &fragment[p_idx..];
        if let Some(gt) = after_p.find('>') {
            let inner = &after_p[gt + 1..];
            if let Some(end_p) = inner.find("</p>") {
                let s = strip_tags_and_entities(&inner[..end_p]);
                if !s.is_empty() {
                    return s.chars().take(260).collect();
                }
            }
        }
    }
    strip_tags_and_entities(&fragment[..fragment.len().min(600)])
        .chars()
        .take(240)
        .collect()
}

/// Unwraps Bing tracking URLs (`https://cn.bing.com/ck/a?...&u=a1aHR0cHM6...`) into clean target URLs.
pub fn unwrap_bing_url(raw: &str) -> String {
    if !raw.contains("bing.com/ck/a") {
        return raw.to_string();
    }
    for param in raw.split(['?', '&']) {
        if let Some(u_val) = param.strip_prefix("u=") {
            if let Some(b64) = u_val.strip_prefix("a1") {
                if let Ok(bytes) = URL_SAFE_NO_PAD
                    .decode(b64)
                    .or_else(|_| URL_SAFE.decode(b64))
                {
                    if let Ok(decoded) = String::from_utf8(bytes) {
                        if decoded.starts_with("http") {
                            return decoded;
                        }
                    }
                }
            }
        }
    }
    raw.to_string()
}

/// Unwraps DuckDuckGo redirect URLs (`//duckduckgo.com/l/?uddg=https%3A%2F%2F...`).
pub fn unwrap_ddg_url(raw: &str) -> String {
    for param in raw.split(['?', '&']) {
        if let Some(uddg) = param.strip_prefix("uddg=") {
            let decoded = url_decode(uddg);
            if decoded.starts_with("http") {
                return decoded;
            }
        }
    }
    if raw.starts_with("//") {
        format!("https:{raw}")
    } else {
        raw.to_string()
    }
}

pub fn clean_html_to_readable_text(html: &str) -> String {
    let mut cleaned = html.to_string();
    for tag in ["script", "style", "noscript", "svg", "nav", "footer", "header"] {
        cleaned = remove_tag_blocks(&cleaned, tag);
    }
    strip_tags_and_entities(&cleaned)
}

fn remove_tag_blocks(input: &str, tag: &str) -> String {
    let open_pat = format!("<{tag}");
    let close_pat = format!("</{tag}>");
    let lower = input.to_ascii_lowercase();

    let mut out = String::with_capacity(input.len());
    let mut idx = 0;
    while let Some(rel_start) = lower[idx..].find(&open_pat) {
        let abs_start = idx + rel_start;
        out.push_str(&input[idx..abs_start]);
        if let Some(rel_end) = lower[abs_start..].find(&close_pat) {
            idx = abs_start + rel_end + close_pat.len();
        } else {
            idx = input.len();
            break;
        }
    }
    out.push_str(&input[idx..]);
    out
}

fn strip_tags_and_entities(html: &str) -> String {
    let mut raw = String::with_capacity(html.len());
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => {
                in_tag = true;
                raw.push(' ');
            }
            '>' => {
                in_tag = false;
            }
            _ if !in_tag => raw.push(ch),
            _ => {}
        }
    }
    let decoded = decode_html_entities(&raw);
    decoded.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn decode_html_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&nbsp;", " ")
}

fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.as_bytes() {
        match *b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char);
            }
            b' ' => out.push('+'),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

fn url_decode(s: &str) -> String {
    let mut bytes = Vec::with_capacity(s.len());
    let raw = s.as_bytes();
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'%' && i + 2 < raw.len() {
            if let Ok(hex) = std::str::from_utf8(&raw[i + 1..i + 3]) {
                if let Ok(val) = u8::from_str_radix(hex, 16) {
                    bytes.push(val);
                    i += 3;
                    continue;
                }
            }
        } else if raw[i] == b'+' {
            bytes.push(b' ');
            i += 1;
            continue;
        }
        bytes.push(raw[i]);
        i += 1;
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

#[cfg(test)]
mod tests {
    use super::{clean_html_to_readable_text, parse_bing_html, unwrap_bing_url, unwrap_ddg_url};

    #[test]
    fn unwraps_bing_and_ddg_redirect_urls() {
        let bing = "https://cn.bing.com/ck/a?!&&p=abc&u=a1aHR0cHM6Ly9ydXN0LWxhbmcub3JnLw&ntb=1";
        assert_eq!(unwrap_bing_url(bing), "https://rust-lang.org/");

        let ddg = "//duckduckgo.com/l/?uddg=https%3A%2F%2Ftauri.app%2Fv2%2F&rut=123";
        assert_eq!(unwrap_ddg_url(ddg), "https://tauri.app/v2/");
    }

    #[test]
    fn parses_bing_result_block_and_cleans_html() {
        let sample = r#"
            <li class="b_algo">
              <h2><a href="https://example.com/guide">Tauri <strong>v2</strong> Guide &amp; Tips</a></h2>
              <div class="b_caption"><p>Build lightweight desktop apps on Windows 11.</p></div>
            </li>
        "#;
        let items = parse_bing_html(sample, 5);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].url, "https://example.com/guide");
        assert_eq!(items[0].title, "Tauri v2 Guide & Tips");
        assert_eq!(items[0].snippet, "Build lightweight desktop apps on Windows 11.");

        let dirty = "<html><script>alert(1)</script><body><h1>Hello</h1><p>World &amp; Rust</p></body></html>";
        assert_eq!(clean_html_to_readable_text(dirty), "Hello World & Rust");
    }
}
