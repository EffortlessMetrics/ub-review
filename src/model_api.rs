//! Model API layer: provider endpoint construction, request payload
//! building, response parsing, error classification, and the curl HTTP
//! transport (cleanup train step 19, pure code motion).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, Stdio};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use wait_timeout::ChildExt;

use crate::*;

pub(crate) fn model_api_key_env(provider: ModelProvider) -> &'static str {
    match provider {
        ModelProvider::MiniMaxDirect => "UB_REVIEW_MINIMAX_API_KEY",
        ModelProvider::OpenCodeGo => "UB_REVIEW_OPENCODE_API_KEY",
    }
}

pub(crate) fn model_api_key_label(provider: ModelProvider) -> &'static str {
    match provider {
        ModelProvider::MiniMaxDirect => "minimax API key",
        ModelProvider::OpenCodeGo => "opencode-go API key",
    }
}

pub(crate) fn model_api_key_present(provider: ModelProvider) -> bool {
    env_value_present(model_api_key_env(provider))
}

pub(crate) fn env_value_present(name: &str) -> bool {
    env_value(name).is_some()
}

pub(crate) fn env_value(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

pub(crate) fn model_api_url(spec: &ProviderSpec) -> String {
    match spec.provider {
        ModelProvider::MiniMaxDirect => {
            if let Ok(value) = std::env::var("UB_REVIEW_MINIMAX_API_URL")
                && !value.trim().is_empty()
            {
                return value;
            }
            match spec.endpoint_kind {
                ProviderEndpointKind::AnthropicMessages => {
                    "https://api.minimax.io/anthropic/v1/messages".to_owned()
                }
                ProviderEndpointKind::OpenAiChat => {
                    "https://api.minimax.io/v1/chat/completions".to_owned()
                }
            }
        }
        ModelProvider::OpenCodeGo => {
            if let Ok(value) = std::env::var("UB_REVIEW_OPENCODE_API_URL")
                && !value.trim().is_empty()
            {
                return value;
            }
            match spec.endpoint_kind {
                ProviderEndpointKind::AnthropicMessages => {
                    "https://opencode.ai/zen/go/v1/messages".to_owned()
                }
                ProviderEndpointKind::OpenAiChat => {
                    "https://opencode.ai/zen/go/v1/chat/completions".to_owned()
                }
            }
        }
    }
}

pub(crate) fn model_auth_header(spec: &ProviderSpec, token: &str) -> String {
    match spec.provider {
        ModelProvider::MiniMaxDirect
            if spec.endpoint_kind == ProviderEndpointKind::AnthropicMessages =>
        {
            format!("X-Api-Key: {token}")
        }
        ModelProvider::MiniMaxDirect => format!("Authorization: Bearer {token}"),
        ModelProvider::OpenCodeGo => format!("Authorization: Bearer {token}"),
    }
}

#[cfg(test)]
pub(crate) fn model_request_payload(spec: &ProviderSpec, prompt: &str) -> serde_json::Value {
    model_request_payload_parts(spec, None, prompt)
}

#[cfg(test)]
pub(crate) fn model_request_payload_parts(
    spec: &ProviderSpec,
    cacheable_prefix: Option<&str>,
    prompt: &str,
) -> serde_json::Value {
    model_request_payload_parts_with_cache_control(spec, cacheable_prefix, prompt, true)
}

pub(crate) fn model_request_payload_parts_with_cache_control(
    spec: &ProviderSpec,
    cacheable_prefix: Option<&str>,
    prompt: &str,
    use_cache_control: bool,
) -> serde_json::Value {
    match spec.endpoint_kind {
        ProviderEndpointKind::AnthropicMessages => {
            let thinking_type = if spec.provider == ModelProvider::MiniMaxDirect {
                "disabled"
            } else {
                "adaptive"
            };
            let content = anthropic_user_content(spec, cacheable_prefix, prompt, use_cache_control);
            serde_json::json!({
                "model": spec.model,
                "max_tokens": model_max_tokens(spec),
                "system": "Return one compact JSON object in the final text block. Do not include markdown fences or prose outside JSON.",
                "thinking": {"type": thinking_type},
                "temperature": 0.1,
                "messages": [
                    {"role": "user", "content": content}
                ],
            })
        }
        ProviderEndpointKind::OpenAiChat if spec.provider == ModelProvider::MiniMaxDirect => {
            let prompt = combined_model_prompt(cacheable_prefix, prompt);
            serde_json::json!({
                "model": spec.model,
                "messages": [
                    {"role": "system", "content": "Return strict JSON only. Do not include markdown fences or prose outside JSON."},
                    {"role": "user", "content": prompt}
                ],
                "max_completion_tokens": model_max_tokens(spec),
                "reasoning_split": true,
                "response_format": {"type": "json_object"},
                "temperature": 0.1,
                "stream": false
            })
        }
        ProviderEndpointKind::OpenAiChat => {
            let prompt = combined_model_prompt(cacheable_prefix, prompt);
            serde_json::json!({
                "model": spec.model,
                "messages": [
                    {"role": "system", "content": "Return strict JSON only. Do not include markdown fences."},
                    {"role": "user", "content": prompt}
                ],
                "temperature": 0.1,
                "stream": false
            })
        }
    }
}

pub(crate) fn anthropic_user_content(
    spec: &ProviderSpec,
    cacheable_prefix: Option<&str>,
    prompt: &str,
    use_cache_control: bool,
) -> serde_json::Value {
    if spec.provider == ModelProvider::MiniMaxDirect
        && use_cache_control
        && let Some(cacheable_prefix) = cacheable_prefix
    {
        return serde_json::json!([
            {
                "type": "text",
                "text": cacheable_prefix,
                "cache_control": {"type": "ephemeral"}
            },
            {
                "type": "text",
                "text": prompt
            }
        ]);
    }
    serde_json::Value::String(combined_model_prompt(cacheable_prefix, prompt))
}

pub(crate) fn combined_model_prompt(cacheable_prefix: Option<&str>, prompt: &str) -> String {
    match cacheable_prefix {
        Some(prefix) => format!("Cached shared context:\n\n{prefix}\n\nLane task:\n\n{prompt}"),
        None => prompt.to_owned(),
    }
}

pub(crate) fn model_max_tokens(spec: &ProviderSpec) -> u32 {
    match spec.endpoint_kind {
        ProviderEndpointKind::AnthropicMessages => 4096,
        ProviderEndpointKind::OpenAiChat => 4096,
    }
}

pub(crate) fn extract_model_content(response: &serde_json::Value) -> Option<&str> {
    response
        .pointer("/choices/0/message/content")
        .and_then(serde_json::Value::as_str)
        .or_else(|| anthropic_content_text(response))
        .or_else(|| response.get("text").and_then(serde_json::Value::as_str))
        .or_else(|| response.get("reply").and_then(serde_json::Value::as_str))
        .or_else(|| response.get("content").and_then(serde_json::Value::as_str))
}

pub(crate) fn anthropic_content_text(response: &serde_json::Value) -> Option<&str> {
    response
        .get("content")?
        .as_array()?
        .iter()
        .find_map(|item| item.get("text").and_then(serde_json::Value::as_str))
}

pub(crate) fn model_response_shape(response: &serde_json::Value) -> &'static str {
    if response.pointer("/choices/0/message/content").is_some() {
        "openai"
    } else if anthropic_content_text(response).is_some() {
        "anthropic"
    } else if response.get("reply").is_some() || response.get("content").is_some() {
        "provider-flat"
    } else {
        "unknown"
    }
}

pub(crate) fn model_cache_usage(response: &serde_json::Value) -> ModelCacheUsage {
    let usage = response.get("usage");
    ModelCacheUsage {
        input_tokens: usage
            .and_then(|usage| usage.get("input_tokens"))
            .and_then(serde_json::Value::as_u64)
            .or_else(|| {
                usage
                    .and_then(|usage| usage.get("prompt_tokens"))
                    .and_then(serde_json::Value::as_u64)
            }),
        output_tokens: usage
            .and_then(|usage| usage.get("output_tokens"))
            .and_then(serde_json::Value::as_u64)
            .or_else(|| {
                usage
                    .and_then(|usage| usage.get("completion_tokens"))
                    .and_then(serde_json::Value::as_u64)
            }),
        cache_creation_input_tokens: usage
            .and_then(|usage| usage.get("cache_creation_input_tokens"))
            .and_then(serde_json::Value::as_u64),
        cache_read_input_tokens: usage
            .and_then(|usage| usage.get("cache_read_input_tokens"))
            .and_then(serde_json::Value::as_u64)
            .or_else(|| {
                usage
                    .and_then(|usage| usage.pointer("/prompt_tokens_details/cached_tokens"))
                    .and_then(serde_json::Value::as_u64)
            }),
    }
}

pub(crate) fn model_json_payload(content: &str) -> String {
    let trimmed = content.trim();
    strip_markdown_json_fence(trimmed)
        .map(str::trim)
        .unwrap_or(content)
        .to_owned()
}

pub(crate) fn strip_markdown_json_fence(trimmed: &str) -> Option<&str> {
    let body = trimmed.strip_prefix("```")?;
    let newline = body.find('\n')?;
    let (info, rest_with_newline) = body.split_at(newline);
    let info = info.trim();
    if !info.is_empty() && !info.eq_ignore_ascii_case("json") {
        return None;
    }
    let rest = rest_with_newline.strip_prefix('\n')?.trim_end();
    rest.strip_suffix("```")
}

pub(crate) fn classify_model_error(err: &anyhow::Error) -> String {
    let text = model_error_chain_text(err).to_ascii_lowercase();
    if text.contains("401") || text.contains("403") || text.contains("auth") {
        "auth_failed".to_owned()
    } else if text.contains("429") || text.contains("rate") {
        "rate_limited".to_owned()
    } else if text.contains("timed out")
        || text.contains("timeout")
        || text.contains("operation timed out")
    {
        "timed_out".to_owned()
    } else if text.contains("parse") {
        "invalid_json".to_owned()
    } else if text.contains("assistant content") {
        "bad_envelope".to_owned()
    } else {
        "failed".to_owned()
    }
}

pub(crate) fn model_error_chain_text(err: &anyhow::Error) -> String {
    format!("{err:#}")
}

pub(crate) fn run_curl_json_post(
    root: &Path,
    url: &str,
    auth_header: &str,
    request_path: &Path,
    headers: &[&str],
    timeout_sec: u64,
) -> Result<HttpPostOutput> {
    run_curl_json_send(
        root,
        "POST",
        url,
        auth_header,
        request_path,
        headers,
        timeout_sec,
    )
}

pub(crate) fn run_curl_json_send(
    root: &Path,
    method: &str,
    url: &str,
    auth_header: &str,
    request_path: &Path,
    headers: &[&str],
    timeout_sec: u64,
) -> Result<HttpPostOutput> {
    run_curl_json_request(
        root,
        method,
        url,
        auth_header,
        Some(request_path),
        headers,
        timeout_sec,
    )
}

pub(crate) fn run_curl_json_request(
    root: &Path,
    method: &str,
    url: &str,
    auth_header: &str,
    request_path: Option<&Path>,
    headers: &[&str],
    timeout_sec: u64,
) -> Result<HttpPostOutput> {
    let fallback_request_path = root.join("ub-review-curl-request");
    let output_anchor = request_path.unwrap_or(&fallback_request_path);
    let data_binary_arg = request_path.map(curl_data_binary_arg).transpose()?;
    let (stdout_path, stderr_path) = curl_temp_output_paths(output_anchor);
    let stdout =
        File::create(&stdout_path).with_context(|| format!("create {}", stdout_path.display()))?;
    let stderr =
        File::create(&stderr_path).with_context(|| format!("create {}", stderr_path.display()))?;
    let mut command = ProcessCommand::new("curl");
    command
        .arg("-sS")
        .arg("--fail-with-body")
        .arg("--max-time")
        .arg(timeout_sec.to_string())
        .arg("-w")
        .arg("\nUB_REVIEW_HTTP_STATUS:%{http_code}\n")
        .arg("-X")
        .arg(method)
        .arg("-K")
        .arg("-");
    if let Some(data_binary_arg) = data_binary_arg {
        command.arg("--data-binary").arg(data_binary_arg);
    }
    command
        .arg(url)
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(err) => {
            remove_output_files(&stdout_path, &stderr_path);
            return Err(err).with_context(|| "spawn curl");
        }
    };
    let write_config_result = (|| -> Result<()> {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow::anyhow!("curl stdin unavailable"))?;
        use std::io::Write as _;
        for header in headers {
            writeln!(stdin, "header = \"{}\"", curl_config_quote(header))?;
        }
        writeln!(stdin, "header = \"{}\"", curl_config_quote(auth_header))?;
        Ok(())
    })();
    if let Err(err) = write_config_result {
        let _ = child.kill();
        let _ = child.wait();
        remove_output_files(&stdout_path, &stderr_path);
        return Err(err);
    }
    let output = wait_for_child_output_files(child, &stdout_path, &stderr_path, timeout_sec)
        .with_context(|| "wait for curl")?;
    let (stdout, http_status) = split_curl_http_status(output.stdout);
    Ok(HttpPostOutput {
        status: output.status,
        stdout,
        stderr: output.stderr,
        http_status,
    })
}

pub(crate) fn curl_temp_output_paths(request_path: &Path) -> (PathBuf, PathBuf) {
    let dir = request_path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = request_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("request.json");
    (
        dir.join(format!("{file_name}.curl.stdout.tmp")),
        dir.join(format!("{file_name}.curl.stderr.tmp")),
    )
}

pub(crate) fn curl_data_binary_arg(request_path: &Path) -> Result<String> {
    let absolute = fs::canonicalize(request_path)
        .with_context(|| format!("canonicalize {}", request_path.display()))?;
    let path = absolute.to_string_lossy().replace('\\', "/");
    Ok(format!("@{path}"))
}

pub(crate) fn wait_for_child_output_files(
    mut child: Child,
    stdout_path: &Path,
    stderr_path: &Path,
    timeout_sec: u64,
) -> Result<FileCommandOutput> {
    let status = match child.wait_timeout(Duration::from_secs(timeout_sec))? {
        Some(status) => status,
        None => {
            let _ = child.kill();
            let _ = child.wait();
            let stdout = read_and_remove_output_file(stdout_path)?;
            let stderr = read_and_remove_output_file(stderr_path)?;
            bail!(
                "process timed out after {timeout_sec}s: stderr: {}; stdout: {}",
                String::from_utf8_lossy(&stderr),
                String::from_utf8_lossy(&stdout)
            );
        }
    };
    let stdout = read_and_remove_output_file(stdout_path)?;
    let stderr = read_and_remove_output_file(stderr_path)?;
    Ok(FileCommandOutput {
        status,
        stdout,
        stderr,
    })
}

pub(crate) fn read_and_remove_output_file(path: &Path) -> Result<Vec<u8>> {
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let _ = fs::remove_file(path);
    Ok(bytes)
}

pub(crate) fn remove_output_files(stdout_path: &Path, stderr_path: &Path) {
    let _ = fs::remove_file(stdout_path);
    let _ = fs::remove_file(stderr_path);
}

pub(crate) fn curl_config_quote(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

pub(crate) fn split_curl_http_status(stdout: Vec<u8>) -> (Vec<u8>, Option<u16>) {
    const MARKER: &[u8] = b"\nUB_REVIEW_HTTP_STATUS:";
    let Some(position) = stdout
        .windows(MARKER.len())
        .rposition(|window| window == MARKER)
    else {
        return (stdout, None);
    };
    let status_bytes = &stdout[position + MARKER.len()..];
    let Ok(status_text) = std::str::from_utf8(status_bytes) else {
        return (stdout, None);
    };
    let Ok(status) = status_text.trim().parse::<u16>() else {
        return (stdout, None);
    };
    let mut body = stdout;
    body.truncate(position);
    (body, Some(status))
}

pub(crate) fn http_status_from_error(err: &anyhow::Error) -> Option<u16> {
    let text = model_error_chain_text(err);
    let needle = "http status Some(";
    let start = text.find(needle)? + needle.len();
    let end = text[start..].find(')')? + start;
    text[start..end].parse::<u16>().ok()
}
