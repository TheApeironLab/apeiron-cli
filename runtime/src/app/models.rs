use super::*;
use std::time::Instant;
pub fn request(model: &Value, action: &str, cancel: &process::Cancellation) -> Result<Value> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| fail("无法启动模型请求。", 500))?;
    runtime.block_on(async {
        tokio::select! {
            _ = cancel.cancelled() => Err(fail("操作已取消。", 499)),
            result = request_inner(model, action) => result,
        }
    })
}
async fn request_inner(model: &Value, action: &str) -> Result<Value> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(45))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| fail("无法启动模型请求。", 500))?;
    let client = &client;
    let call = |path: &'static str, body: Option<Value>| async move {
        let url = format!("{}/{}", model["baseUrl"].as_str().unwrap(), path);
        let mut req = if let Some(body) = body {
            client.post(url).json(&body)
        } else {
            client.get(url)
        };
        req = req.header("Content-Type", "application/json");
        if let Some(key) = model["apiKey"].as_str().filter(|s| !s.is_empty()) {
            req = req.bearer_auth(key);
        }
        let mut response = req
            .send()
            .await
            .map_err(|_| fail("无法完成模型请求：检查网络、地址和接口兼容性（45 秒超时）。", 400))?;
        if response.status().is_redirection() {
            return Err(fail("无法完成模型请求：模型服务不允许重定向。", 400));
        }
        if !response.status().is_success() {
            return Err(fail(
                format!(
                    "模型服务返回 HTTP {}，请检查地址、密钥和模型权限。",
                    response.status().as_u16()
                ),
                400,
            ));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| fail("无法读取模型响应。", 400))? {
            if bytes.len() + chunk.len() > 1_048_576 {
                return Err(fail("模型响应超过大小限制。", 400));
            }
            bytes.extend_from_slice(&chunk);
        }
        serde_json::from_slice::<Value>(&bytes).map_err(|_| fail("模型响应格式不正确。", 400))
    };
    if action == "list" {
        let data = call("models", None).await?;
        let rows = data["data"]
            .as_array()
            .ok_or_else(|| fail("模型服务未返回有效模型列表。", 400))?;
        let mut models = Vec::new();
        for row in rows {
            if let Some(id) = row["id"]
                .as_str()
                .filter(|id| !id.is_empty() && id.len() <= 256 && !id.chars().any(char::is_control))
            {
                if !models.contains(&id) {
                    models.push(id);
                }
                if models.len() == 500 {
                    break;
                }
            }
        }
        if models.is_empty() {
            return Err(fail("模型服务未返回可用模型。", 400));
        }
        return Ok(json!({"models":models}));
    }
    let mut results = Vec::new();
    for mode in ["fast", "deep"] {
        let start = Instant::now();
        let data = call(
            "chat/completions",
            Some(
                json!({"model":model[mode],"messages":[{"role":"user","content":"Reply OK."}],"max_tokens":64,"stream":false}),
            ),
        ).await?;
        let message = &data["choices"][0]["message"];
        if ["content", "reasoning_content"]
            .iter()
            .all(|k| message[k].as_str().unwrap_or("").is_empty())
        {
            return Err(fail("模型返回了空响应或不兼容的响应。", 400));
        }
        results.push(json!({"mode":mode,"model":model[mode],"elapsedMs":start.elapsed().as_millis()}));
    }
    Ok(json!({"results":results}))
}
pub fn values(model: &Value) -> Value {
    let mut queue = vec![model["fast"].clone()];
    if model["deep"] != model["fast"] {
        queue.push(model["deep"].clone());
    }
    json!({"providers":[{"id":model["provider"],"baseUrl":model["baseUrl"],"apiKeyEnv":"MODEL_API_KEY_SETUP"}],"models":[{"id":"apeiron-flash","provider":model["provider"],"model":model["fast"],"label":"apeiron-flash"},{"id":"apeiron-pro","provider":model["provider"],"model":model["deep"],"label":"apeiron-pro"}],"modes":{"fast":"apeiron-flash","deep":"apeiron-pro"},"queues":queue.iter().map(|m|json!({"provider":model["provider"],"model":m,"maxConcurrency":4,"maxPending":16})).collect::<Vec<_>>()})
}
