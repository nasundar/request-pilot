use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
struct AzTokenResponse {
    #[serde(rename = "accessToken")]
    access_token: String,
    #[serde(rename = "expiresOn")]
    expires_on: String,
    #[allow(dead_code)]
    subscription: Option<String>,
    tenant: Option<String>,
    #[allow(dead_code)]
    #[serde(rename = "tokenType")]
    token_type: Option<String>,
}

#[derive(Debug, serde::Serialize)]
pub struct AzureToken {
    pub access_token: String,
    pub expires_on: String,
    pub tenant: Option<String>,
}

/// Fetch an access token for the given resource using the Azure CLI.
/// Requires `az` CLI installed and user logged in (`az login`).
pub fn fetch_token(resource: &str) -> Result<AzureToken, String> {
    let commands = if cfg!(windows) {
        vec!["az.cmd", "az"]
    } else {
        vec!["az"]
    };
    
    let mut last_error = String::from("az CLI not found");
    for cmd in commands {
        match std::process::Command::new(cmd)
            .args(["account", "get-access-token", "--resource", resource])
            .output()
        {
            Ok(output) => {
                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    last_error = format!("Azure CLI error: {}. Run 'az login' to authenticate.", stderr.trim());
                    continue;
                }
                let stdout = String::from_utf8(output.stdout)
                    .map_err(|e| format!("Invalid UTF-8 in az output: {}", e))?;
                let resp: AzTokenResponse = serde_json::from_str(&stdout)
                    .map_err(|e| format!("Failed to parse az JSON response: {}", e))?;
                return Ok(AzureToken {
                    access_token: resp.access_token,
                    expires_on: resp.expires_on,
                    tenant: resp.tenant,
                });
            }
            Err(_) => continue,
        }
    }
    Err(last_error)
}

/// Check if Azure CLI is available on the system.
pub fn is_az_cli_available() -> bool {
    // On Windows, try both 'az' and 'az.cmd' since PATH resolution differs
    let commands = if cfg!(windows) {
        vec!["az.cmd", "az"]
    } else {
        vec!["az"]
    };
    for cmd in commands {
        if std::process::Command::new(cmd)
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
        {
            return true;
        }
    }
    false
}

// --- OAuth2 Device Code Flow ---

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DeviceCodeResponse {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
    pub interval: u64,
    pub message: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TokenResponse {
    pub access_token: String,
    pub token_type: String,
    pub expires_in: u64,
    pub scope: Option<String>,
}

/// Initiate device code flow. Returns device code info for user to authenticate.
pub async fn start_device_code_flow(
    tenant_id: &str,
    client_id: &str,
    scope: &str,
) -> Result<DeviceCodeResponse, String> {
    let url = format!(
        "https://login.microsoftonline.com/{}/oauth2/v2.0/devicecode",
        tenant_id
    );

    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .form(&[("client_id", client_id), ("scope", scope)])
        .send()
        .await
        .map_err(|e| format!("Device code request failed: {}", e))?;

    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("Device code request failed: {}", body));
    }

    resp.json::<DeviceCodeResponse>()
        .await
        .map_err(|e| format!("Failed to parse device code response: {}", e))
}

/// Poll for token after user authenticates. Returns token or polling status.
pub async fn poll_device_code_token(
    tenant_id: &str,
    client_id: &str,
    device_code: &str,
) -> Result<Result<TokenResponse, String>, String> {
    let url = format!(
        "https://login.microsoftonline.com/{}/oauth2/v2.0/token",
        tenant_id
    );

    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .form(&[
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ("client_id", client_id),
            ("device_code", device_code),
        ])
        .send()
        .await
        .map_err(|e| format!("Token poll failed: {}", e))?;

    let body = resp.text().await.unwrap_or_default();

    // Try to parse as success token
    if let Ok(token) = serde_json::from_str::<TokenResponse>(&body) {
        return Ok(Ok(token));
    }

    // Check for pending/error states
    if body.contains("authorization_pending") {
        return Ok(Err("authorization_pending".to_string()));
    }
    if body.contains("slow_down") {
        return Ok(Err("slow_down".to_string()));
    }
    if body.contains("expired_token") {
        return Err("Device code expired. Please try again.".to_string());
    }
    if body.contains("authorization_declined") {
        return Err("Authorization was declined by the user.".to_string());
    }

    Err(format!("Unexpected token response: {}", body))
}
