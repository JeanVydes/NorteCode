use crate::api::ApiError;
use raep_types::Envelope;
use serde_json::Value;

#[derive(Default)]
struct SseDecoder {
    buffer: Vec<u8>,
}

impl SseDecoder {
    fn push(&mut self, chunk: &[u8]) {
        self.buffer.extend_from_slice(chunk);
    }

    fn next_data(&mut self) -> Result<Option<String>, ApiError> {
        loop {
            let Some((end, delimiter_len)) = self.next_boundary() else {
                return Ok(None);
            };
            let event: Vec<u8> = self.buffer.drain(..end).collect();
            self.buffer.drain(..delimiter_len);
            let event = std::str::from_utf8(&event)
                .map_err(|error| ApiError::InvalidResponse(error.to_string()))?;
            let data = event
                .lines()
                .filter_map(|line| line.strip_prefix("data:"))
                .map(|line| line.strip_prefix(' ').unwrap_or(line))
                .collect::<Vec<_>>()
                .join("\n");
            if !data.is_empty() {
                return Ok(Some(data));
            }
        }
    }

    fn next_boundary(&self) -> Option<(usize, usize)> {
        let lf = self
            .buffer
            .windows(2)
            .position(|window| window == b"\n\n")
            .map(|index| (index, 2));
        let crlf = self
            .buffer
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .map(|index| (index, 4));
        match (lf, crlf) {
            (Some(lf), Some(crlf)) => Some(if lf.0 <= crlf.0 { lf } else { crlf }),
            (Some(boundary), None) | (None, Some(boundary)) => Some(boundary),
            (None, None) => None,
        }
    }
}

pub(crate) struct SessionEventStream {
    #[cfg(target_arch = "wasm32")]
    reader: web_sys::ReadableStreamDefaultReader,
    #[cfg(target_arch = "wasm32")]
    abort: web_sys::AbortController,
    #[cfg(not(target_arch = "wasm32"))]
    response: reqwest::Response,
    decoder: SseDecoder,
}

impl SessionEventStream {
    #[cfg(target_arch = "wasm32")]
    pub(crate) async fn connect(url: &str, bearer_token: Option<&str>) -> Result<Self, ApiError> {
        use wasm_bindgen::{JsCast, JsValue};
        use wasm_bindgen_futures::JsFuture;
        use web_sys::{
            AbortController, Headers, ReadableStreamDefaultReader, Request, RequestInit,
        };

        let headers = Headers::new().map_err(|error| js_error(&error))?;
        headers
            .set("Accept", "text/event-stream")
            .map_err(|error| js_error(&error))?;
        if let Some(token) = bearer_token.filter(|token| !token.is_empty()) {
            headers
                .set("Authorization", &format!("Bearer {token}"))
                .map_err(|error| js_error(&error))?;
        }

        let abort = AbortController::new().map_err(|error| js_error(&error))?;
        let options = RequestInit::new();
        options.set_method("GET");
        options.set_headers(&headers);
        options.set_signal(Some(&abort.signal()));
        let request =
            Request::new_with_str_and_init(url, &options).map_err(|error| js_error(&error))?;
        let window = web_sys::window()
            .ok_or_else(|| ApiError::Request("The browser window is unavailable.".to_owned()))?;
        let response = JsFuture::from(window.fetch_with_request(&request))
            .await
            .map_err(|error| js_error(&error))?
            .dyn_into::<web_sys::Response>()
            .map_err(|error| js_error(&error))?;
        if !response.ok() {
            let status = response.status();
            let detail = JsFuture::from(response.text().map_err(|error| js_error(&error))?)
                .await
                .ok()
                .and_then(|value| value.as_string())
                .filter(|value| !value.trim().is_empty())
                .map_or_else(String::new, |value| format!(": {value}"));
            return Err(ApiError::InvalidResponse(format!(
                "The session event stream returned HTTP {status}{detail}"
            )));
        }

        let body = response.body().ok_or_else(|| {
            ApiError::InvalidResponse("The session event stream has no response body.".to_owned())
        })?;
        let reader = body
            .get_reader()
            .dyn_into::<ReadableStreamDefaultReader>()
            .map_err(|error| {
                let value: JsValue = error.into();
                js_error(&value)
            })?;
        Ok(Self {
            reader,
            abort,
            decoder: SseDecoder::default(),
        })
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) async fn connect(
        client: &reqwest::Client,
        url: &str,
        bearer_token: Option<&str>,
    ) -> Result<Self, ApiError> {
        let mut request = client.get(url).header("Accept", "text/event-stream");
        if let Some(token) = bearer_token.filter(|token| !token.is_empty()) {
            request = request.bearer_auth(token);
        }
        let response = request
            .send()
            .await
            .map_err(|error| ApiError::Request(error.to_string()))?;
        if !response.status().is_success() {
            let status = response.status();
            let detail = response
                .text()
                .await
                .unwrap_or_else(|error| error.to_string());
            return Err(ApiError::InvalidResponse(format!(
                "The session event stream returned HTTP {status}: {detail}"
            )));
        }
        Ok(Self {
            response,
            decoder: SseDecoder::default(),
        })
    }

    pub(crate) async fn next(&mut self) -> Result<Option<Envelope<Value>>, ApiError> {
        #[cfg(target_arch = "wasm32")]
        use js_sys::{Reflect, Uint8Array};
        #[cfg(target_arch = "wasm32")]
        use wasm_bindgen::JsValue;
        #[cfg(target_arch = "wasm32")]
        use wasm_bindgen_futures::JsFuture;

        loop {
            if let Some(data) = self.decoder.next_data()? {
                let frame = serde_json::from_str(&data)
                    .map_err(|error| ApiError::InvalidResponse(error.to_string()))?;
                return Ok(Some(frame));
            }

            #[cfg(target_arch = "wasm32")]
            {
                let result = JsFuture::from(self.reader.read())
                    .await
                    .map_err(|error| js_error(&error))?;
                let done = Reflect::get(&result, &JsValue::from_str("done"))
                    .map_err(|error| js_error(&error))?
                    .as_bool()
                    .unwrap_or(false);
                if done {
                    return Ok(None);
                }
                let chunk = Reflect::get(&result, &JsValue::from_str("value"))
                    .map_err(|error| js_error(&error))?;
                if chunk.is_undefined() || chunk.is_null() {
                    continue;
                }
                let chunk = Uint8Array::new(&chunk);
                let length = usize::try_from(chunk.length())
                    .map_err(|error| ApiError::InvalidResponse(error.to_string()))?;
                let mut bytes = vec![0; length];
                chunk.copy_to(&mut bytes);
                self.decoder.push(&bytes);
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                let Some(chunk) = self
                    .response
                    .chunk()
                    .await
                    .map_err(|error| ApiError::Request(error.to_string()))?
                else {
                    return Ok(None);
                };
                self.decoder.push(&chunk);
            }
        }
    }
}

#[cfg(target_arch = "wasm32")]
impl Drop for SessionEventStream {
    fn drop(&mut self) {
        self.abort.abort();
    }
}

#[cfg(target_arch = "wasm32")]
fn js_error(error: &wasm_bindgen::JsValue) -> ApiError {
    ApiError::Request(error.as_string().unwrap_or_else(|| format!("{error:?}")))
}

#[cfg(test)]
mod tests {
    use super::SseDecoder;

    #[test]
    fn decodes_complete_event_after_split_chunks() {
        let mut decoder = SseDecoder::default();
        decoder.push(b"event: TOKEN_DELTA\ndata: {\"delta\":");
        assert_eq!(decoder.next_data().expect("valid partial frame"), None);
        decoder.push(b"\"hello\"}\n\n");
        assert_eq!(
            decoder.next_data().expect("complete frame"),
            Some("{\"delta\":\"hello\"}".to_owned())
        );
    }

    #[test]
    fn supports_crlf_and_ignores_comment_events() {
        let mut decoder = SseDecoder::default();
        decoder.push(b": keepalive\r\n\r\ndata: {\"value\":1}\r\n\r\n");
        assert_eq!(
            decoder.next_data().expect("complete frame"),
            Some("{\"value\":1}".to_owned())
        );
    }
}
