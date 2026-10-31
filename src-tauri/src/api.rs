use reqwest::blocking::{multipart, Client};
use reqwest::StatusCode;
use serde::Deserialize;
use std::path::Path;
use std::time::Duration;

// This is ChatGPT's web transcription endpoint; it is not the public API.
const TRANSCRIBE_URL: &str = "https://chatgpt.com/backend-api/transcribe";

pub struct TranscribeClient {
    token: String,
    build_number: String,
    client_version: String,
    device_id: String,
    client: Client,
    url: String,
    retry_delay: Duration,
}

#[derive(Deserialize)]
pub struct TranscribeResult {
    pub text: String,
}

impl TranscribeClient {
    pub fn from_fields(
        token: String,
        build_number: String,
        client_version: String,
        device_id: String,
    ) -> Result<Self, String> {
        let client = Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .map_err(|e| format!("無法建立連線: {e}"))?;
        Ok(Self {
            token,
            build_number,
            client_version,
            device_id,
            client,
            url: TRANSCRIBE_URL.into(),
            retry_delay: Duration::from_secs(2),
        })
    }

    pub fn transcribe(&self, path: &Path) -> Result<TranscribeResult, String> {
        let audio = std::fs::read(path).map_err(|e| format!("無法讀取錄音: {e}"))?;
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("audio.wav");
        for attempt in 0..3 {
            if attempt > 0 {
                std::thread::sleep(self.retry_delay);
            }
            let part = multipart::Part::bytes(audio.clone())
                .file_name(file_name.to_string())
                .mime_str("audio/wav")
                .map_err(|e| e.to_string())?;
            let mut request = self
                .client
                .post(&self.url)
                .bearer_auth(&self.token)
                .header("accept", "application/json")
                .header("accept-language", "zh-Hant,zh;q=0.9,en;q=0.8")
                .header("origin", "https://chatgpt.com")
                .header("referer", "https://chatgpt.com/")
                .header("oai-language", "zh-TW");
            #[cfg(target_os = "macos")]
            {
                request = request.header("user-agent", "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36")
                .header("sec-ch-ua-platform", "\"macOS\"");
            }
            #[cfg(not(target_os = "macos"))]
            {
                request = request.header("user-agent", "GPT-Typeless/1.0.1");
            }
            for (name, value) in [
                ("oai-client-build-number", &self.build_number),
                ("oai-client-version", &self.client_version),
                ("oai-device-id", &self.device_id),
            ] {
                if !value.is_empty() {
                    request = request.header(name, value);
                }
            }
            let response = match request
                .multipart(multipart::Form::new().part("file", part))
                .send()
            {
                Ok(response) => response,
                Err(error) if (error.is_timeout() || error.is_connect()) && attempt < 2 => continue,
                Err(_) => return Err("無法連接 ChatGPT，請檢查網絡後重試".into()),
            };
            let status = response.status();
            if (status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error()) && attempt < 2
            {
                continue;
            }
            if !status.is_success() {
                return Err(status_message(status));
            }
            // Do not log response bodies: they can contain private speech.
            return response
                .json::<TranscribeResult>()
                .map_err(|_| "ChatGPT 回應格式已改變或轉錄未成功，錄音已保留".into());
        }
        unreachable!()
    }
}

fn status_message(status: StatusCode) -> String {
    match status {
        StatusCode::UNAUTHORIZED => "ChatGPT Token 已失效（401），請重新登入 ChatGPT 並更新 Token".into(),
        StatusCode::FORBIDDEN => "ChatGPT 拒絕轉錄（403），請確認帳戶可用語音輸入、更新 Token 及選填嘅請求參數；網頁介面亦可能已改變".into(),
        StatusCode::TOO_MANY_REQUESTS => "ChatGPT 用量或請求次數已達上限（429），請稍後再試".into(),
        _ => format!("ChatGPT 轉錄失敗（HTTP {}），錄音已保留", status.as_u16()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    fn mock_server(status: &str, body: &str) -> (String, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/transcribe", listener.local_addr().unwrap());
        let reply = format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
        let thread = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut data = Vec::new();
            loop {
                let mut chunk = [0; 4096];
                let n = stream.read(&mut chunk).unwrap();
                if n == 0 {
                    break;
                }
                data.extend_from_slice(&chunk[..n]);
                if let Some(header_end) = data.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&data[..header_end]).to_lowercase();
                    let length: usize = headers
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length:"))
                        .unwrap()
                        .trim()
                        .parse()
                        .unwrap();
                    if data.len() >= header_end + 4 + length {
                        break;
                    }
                }
            }
            stream.write_all(reply.as_bytes()).unwrap();
            String::from_utf8_lossy(&data).into_owned()
        });
        (url, thread)
    }

    fn sample_file(suffix: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "gpttypeless-api-{}-{suffix}.wav",
            std::process::id()
        ));
        std::fs::write(&path, b"local test audio").unwrap();
        path
    }

    #[test]
    fn uploads_audio_and_reads_transcription_without_remote_service() {
        let (url, server) = mock_server("200 OK", r#"{"text":"測試轉錄"}"#);
        let mut client = TranscribeClient::from_fields(
            "fixture-token".into(),
            "123".into(),
            String::new(),
            String::new(),
        )
        .unwrap();
        client.url = url;
        let path = sample_file("ok");
        assert_eq!(client.transcribe(&path).unwrap().text, "測試轉錄");
        let request = server.join().unwrap();
        assert!(request.contains("Bearer fixture-token"));
        assert!(request.contains("name=\"file\""));
        assert!(request.contains("audio/wav"));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn expired_token_is_reported_without_retry_or_response_body() {
        let (url, server) = mock_server("401 Unauthorized", "private-response");
        let mut client = TranscribeClient::from_fields(
            "fixture-token".into(),
            String::new(),
            String::new(),
            String::new(),
        )
        .unwrap();
        client.url = url;
        let path = sample_file("401");
        let error = client.transcribe(&path).err().unwrap();
        assert!(error.contains("401"));
        assert!(!error.contains("private-response"));
        server.join().unwrap();
        std::fs::remove_file(path).unwrap();
    }
}
