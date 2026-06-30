use anyhow::{Context, Result, anyhow};
use base64::{Engine as _, engine::general_purpose};
use rand::Rng;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

use crate::crypto;

const SHEETA_CERT_ID: &str = "secureLink";
const DEFAULT_API_BASE: &str = "https://svpnlink.xmu.edu.cn";
const DEFAULT_ORG: &str = "xmu";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Session {
    pub two_factor_token: Option<String>,
    pub two_factor_token_expire: Option<i64>,
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
    pub access_token_expire: Option<i64>,
    pub sl_server_type: Option<String>,
    pub base_url: Option<String>,
    pub org: Option<String>,
}

pub struct SecureLinkApi {
    client: Client,
    pub base_url: String,
    pub org: String,
    pub token: Option<String>,
    pub session: Session,
    session_path: PathBuf,
    device_id_path: PathBuf,
    nonce: u32,
}

impl SecureLinkApi {
    pub fn new(data_dir: &Path) -> Result<Self> {
        fs::create_dir_all(data_dir)?;
        Ok(Self {
            client: Client::builder()
                .cookie_store(true)
                .user_agent("SecureLink/3.8.1 (Windows NT 10.0; Win64; x64)")
                .build()?,
            base_url: DEFAULT_API_BASE.to_string(),
            org: DEFAULT_ORG.to_string(),
            token: None,
            session: Session::default(),
            session_path: data_dir.join("session.json"),
            device_id_path: data_dir.join("device_id"),
            nonce: rand::thread_rng().gen_range(1_000_000_000..=u32::MAX),
        })
    }

    pub fn load_session(&mut self) -> Result<bool> {
        if !self.session_path.exists() {
            return Ok(false);
        }
        let data = fs::read_to_string(&self.session_path)?;
        self.session = serde_json::from_str(&data)?;
        if let Some(base_url) = &self.session.base_url {
            self.base_url = base_url.clone();
        }
        if let Some(org) = &self.session.org {
            self.org = org.clone();
        }
        self.token = self.session.access_token.clone();
        Ok(self.token.is_some())
    }

    pub fn save_session(&self) -> Result<()> {
        fs::write(
            &self.session_path,
            serde_json::to_string_pretty(&self.session)?,
        )?;
        Ok(())
    }

    pub async fn login(&mut self, callback_url: Option<String>) -> Result<()> {
        self.load_session()?;
        if callback_url.is_some() {
            self.sso_login_flow(callback_url).await?;
            self.save_session()?;
            return Ok(());
        }

        if self.session.access_token.is_some() && !self.is_access_token_expired() {
            println!("[session] Access token valid");
            return Ok(());
        }

        if self.session.refresh_token.is_some() && self.refresh_if_needed().await? {
            return Ok(());
        }

        self.sso_login_flow(None).await?;
        self.save_session()?;
        Ok(())
    }

    pub async fn get_auth_config(&mut self) -> Result<Value> {
        let body = sorted_json(&json!({
            "authConfigId": Value::Null,
            "org": self.org,
            "version": 0,
        }))?;
        let url = format!(
            "{}/authApi/sso/authConfig/getAuthConfig?locale=zh_CN&locale=zh_CN",
            self.base_url
        );
        self.sheeta_post(&url, &body, true, "5.14.0.0").await
    }

    pub async fn get_auth_url(&mut self, auth_name: &str) -> Result<Value> {
        let body = sorted_json(&json!({
            "authConfigId": Value::Null,
            "name": auth_name,
            "org": self.org,
        }))?;
        let url = format!(
            "{}/authApi/sso/authConfig/getAuthUrl?locale=zh_CN&locale=zh_CN",
            self.base_url
        );
        self.sheeta_post(&url, &body, true, "5.14.0.0").await
    }

    pub async fn validate_code(&mut self, auth_name: &str, code: &str) -> Result<Value> {
        let body = sorted_json(&json!({
            "authConfigId": Value::Null,
            "code": code,
            "name": auth_name,
            "org": self.org,
            "system": "0",
        }))?;
        let url = format!(
            "{}/authApi/sso/authConfig/validateCode?locale=zh_CN&locale=zh_CN",
            self.base_url
        );
        let result = self.sheeta_post(&url, &body, true, "5.14.0.0").await?;
        if return_code(&result) == 1
            && let Some(token) = result
                .pointer("/content/token")
                .and_then(Value::as_str)
                .map(str::to_string)
        {
            self.session.two_factor_token_expire = crypto::decode_jwt_payload(&token)
                .ok()
                .and_then(|v| v.get("exp").and_then(Value::as_i64));
            self.session.two_factor_token = Some(token);
        }
        Ok(result)
    }

    pub async fn sso_login(&mut self) -> Result<Value> {
        let body = sorted_json(&self.login_body()?)?;
        let url = format!("{}/authApi/is/sso/login?locale=zh_CN", self.base_url);
        let saved = self.token.clone();
        self.token = self.session.two_factor_token.clone();
        let result = self.sheeta_post(&url, &body, true, "5.14.0.0").await;
        if self.session.access_token.is_none() {
            self.token = saved;
        }
        let result = result?;
        if return_code(&result) == 1 {
            self.capture_login_tokens(&result)?;
        }
        Ok(result)
    }

    pub async fn sso_login_flow(&mut self, callback_url: Option<String>) -> Result<()> {
        let auth_config = self.get_auth_config().await?;
        if return_code(&auth_config) != 1 {
            return Err(anyhow!(
                "getAuthConfig failed: {}",
                auth_config.get("returnMsg").unwrap_or(&auth_config)
            ));
        }
        let providers = auth_config
            .pointer("/content/list")
            .and_then(Value::as_array)
            .context("auth provider list missing")?;
        let provider = providers
            .iter()
            .find(|p| p.get("type").and_then(Value::as_i64) == Some(1))
            .or_else(|| providers.first())
            .context("no SSO auth provider found")?;
        let auth_name = provider
            .get("name")
            .and_then(Value::as_str)
            .context("auth provider name missing")?
            .to_string();

        let auth_url = self.get_auth_url(&auth_name).await?;
        if return_code(&auth_url) != 1 {
            return Err(anyhow!(
                "getAuthUrl failed: {}",
                auth_url.get("returnMsg").unwrap_or(&auth_url)
            ));
        }
        let login_url = auth_url
            .pointer("/content/loginUrl")
            .and_then(Value::as_str)
            .context("loginUrl missing")?;
        let callback_url = match callback_url.or_else(|| std::env::var("SL_CALLBACK_URL").ok()) {
            Some(url) => url,
            None => prompt_callback_url(login_url)?,
        };
        let code = extract_query_param(&callback_url, "code")
            .context("callback URL does not contain code parameter")?;
        let result = self.validate_code(&auth_name, &code).await?;
        if return_code(&result) != 1 {
            return Err(anyhow!(
                "validateCode failed: {}",
                result.get("returnMsg").unwrap_or(&result)
            ));
        }
        let result = self.sso_login().await?;
        if return_code(&result) != 1 {
            return Err(anyhow!(
                "sso_login failed: {}",
                result.get("returnMsg").unwrap_or(&result)
            ));
        }
        Ok(())
    }

    pub async fn refresh_if_needed(&mut self) -> Result<bool> {
        if !self.is_access_token_expired() {
            return Ok(true);
        }
        let Some(refresh) = self.session.refresh_token.clone() else {
            return Ok(false);
        };
        let saved = self.token.clone();
        self.token = Some(refresh);
        let body = sorted_json(&json!({"org": self.org, "type": 1}))?;
        let url = format!("{}/authApi/is/code/refreshToken", self.base_url);
        let result = self.sheeta_post(&url, &body, true, "4.26.0.0").await;
        self.token = saved;
        let result = result?;
        if return_code(&result) != 1 {
            return Ok(false);
        }
        self.capture_login_tokens(&result)?;
        self.save_session()?;
        println!("[session] Access token refreshed");
        Ok(true)
    }

    pub async fn get_vpn_config(&mut self) -> Result<Value> {
        let _ = self.sl_server_type()?;
        let body = sorted_json(&json!({
            "dns": "",
            "intranetIp": "127.0.0.1",
            "system": 0,
            "wifiSsid": Value::Null,
        }))?;
        let url = format!(
            "{}/networkApi/is/network/initConfig?locale=zh_CN",
            self.base_url
        );
        self.sheeta_post(&url, &body, true, "5.0.0.0").await
    }

    pub fn access_token(&self) -> Result<&str> {
        self.session
            .access_token
            .as_deref()
            .or(self.token.as_deref())
            .context("access token missing")
    }

    pub fn sl_server_type(&mut self) -> Result<String> {
        if let Some(value) = &self.session.sl_server_type {
            return Ok(value.clone());
        }
        let generated = self.generate_sl_server_type()?;
        self.session.sl_server_type = Some(generated.clone());
        Ok(generated)
    }

    fn is_access_token_expired(&mut self) -> bool {
        let Some(token) = self.session.access_token.as_deref() else {
            return true;
        };
        if self.session.access_token_expire.is_none() {
            self.session.access_token_expire = crypto::decode_jwt_payload(token)
                .ok()
                .and_then(|v| v.get("exp").and_then(Value::as_i64));
        }
        let Some(exp) = self.session.access_token_expire else {
            return true;
        };
        unix_now() > exp - 300
    }

    fn capture_login_tokens(&mut self, result: &Value) -> Result<()> {
        let content = result.get("content").cloned().unwrap_or(Value::Null);
        let login_msg = content.get("loginMessage").cloned().unwrap_or(Value::Null);
        let access = pick_string(&[&login_msg, &content], &["accessToken", "token"]);
        if let Some(access) = access {
            self.token = Some(access.clone());
            self.session.access_token = Some(access.clone());
            if self.session.access_token_expire.is_none() {
                self.session.access_token_expire = crypto::decode_jwt_payload(&access)
                    .ok()
                    .and_then(|v| v.get("exp").and_then(Value::as_i64));
            }
        }
        self.session.refresh_token = pick_string(&[&login_msg, &content], &["refreshToken"])
            .or_else(|| self.session.refresh_token.clone());
        if let Some(exp_ms) = pick_i64(&[&login_msg, &content], &["accessTokenExpire"]) {
            self.session.access_token_expire = Some(exp_ms / 1000);
        }
        self.session.sl_server_type =
            pick_string(&[&login_msg, &content], &["slServerType", "serverType"])
                .or_else(|| self.session.sl_server_type.clone());
        self.session.base_url = Some(self.base_url.clone());
        self.session.org = Some(self.org.clone());
        if self.session.access_token.is_none() {
            return Err(anyhow!("login response did not include access token"));
        }
        Ok(())
    }

    fn generate_sl_server_type(&self) -> Result<String> {
        let body = sorted_json(&json!({
            "serverType": "SDP",
            "timestamp": millis_now().to_string(),
        }))?;
        crypto::rsa_encrypt_base64(body.as_bytes())
    }

    async fn sheeta_post(
        &mut self,
        url: &str,
        body: &str,
        encrypt: bool,
        api_version: &str,
    ) -> Result<Value> {
        let req_timestamp = millis_now().to_string();
        self.nonce = self.nonce.wrapping_add(1);
        let nonce = self.nonce.to_string();

        let (body_to_send, session_key, secret) = if encrypt {
            let (enc_body, key, secret) = crypto::encrypt_body(body)?;
            (enc_body, Some(key), Some(secret))
        } else {
            (body.to_string(), None, None)
        };
        let sign =
            crypto::compute_sheeta_sign(SHEETA_CERT_ID, &req_timestamp, &nonce, &body_to_send);

        let mut req = self
            .client
            .post(url)
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .header("apiVersion", api_version)
            .header("certId", SHEETA_CERT_ID)
            .header("reqTimestamp", req_timestamp)
            .header("nonce", nonce)
            .header("sheetaSign", sign);
        if let Some(secret) = secret {
            req = req.header("secret", secret);
        }
        if let Some(sl_server_type) = &self.session.sl_server_type {
            req = req.header("SlServerType", sl_server_type);
        }
        if let Some(token) = &self.token {
            req = req.bearer_auth(token);
        }

        let text = req.body(body_to_send).send().await?.text().await?;
        if let Some(key) = session_key
            && let Ok(ciphertext) = general_purpose::STANDARD.decode(text.trim())
            && let Ok(plain) = crypto::aes_128_cbc_decrypt(&key, crypto::BODY_AES_IV, &ciphertext)
            && let Ok(value) = serde_json::from_slice::<Value>(&plain)
        {
            return Ok(value);
        }
        Ok(serde_json::from_str(&text)?)
    }

    fn device_id(&self) -> Result<String> {
        if self.device_id_path.exists() {
            return Ok(fs::read_to_string(&self.device_id_path)?.trim().to_string());
        }
        let id = Uuid::new_v4().simple().to_string();
        fs::write(&self.device_id_path, &id)?;
        Ok(id)
    }

    fn login_body(&self) -> Result<Value> {
        Ok(json!({
            "archType": 0,
            "deviceId": self.device_id()?,
            "deviceMac": "00:00:00:00:00:00",
            "deviceModel": "x86_64",
            "deviceName": "PC",
            "org": self.org,
            "password": "ZXJyb3I=",
            "system": "0",
            "systemBits": "64",
            "systemModel": "Windows 10",
            "systemName": "Windows 10",
            "version": "3.8.1",
        }))
    }
}

pub fn return_code(value: &Value) -> i64 {
    value
        .get("returnCode")
        .and_then(Value::as_i64)
        .unwrap_or_default()
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

fn millis_now() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis()
}

fn sorted_json(value: &Value) -> Result<String> {
    Ok(serde_json::to_string(value)?)
}

fn pick_string(containers: &[&Value], keys: &[&str]) -> Option<String> {
    for container in containers {
        for key in keys {
            if let Some(value) = container.get(*key).and_then(Value::as_str) {
                return Some(value.to_string());
            }
        }
    }
    None
}

fn pick_i64(containers: &[&Value], keys: &[&str]) -> Option<i64> {
    for container in containers {
        for key in keys {
            if let Some(value) = container.get(*key).and_then(Value::as_i64) {
                return Some(value);
            }
        }
    }
    None
}

/// Open the SSO URL in the system browser and read the pasted callback URL from
/// stdin — the same flow as MySecureLink/app.py `do_login`.
fn prompt_callback_url(login_url: &str) -> Result<String> {
    println!("\n=======================================================");
    println!("  Opening the SecureLink SSO page in your browser.");
    println!("  Finish login, then paste the final callback URL here.");
    println!("  If the browser did not open, visit this URL manually:");
    println!("  {login_url}");
    println!("=======================================================");
    open_browser(login_url);

    print!("  Callback URL: ");
    use std::io::Write;
    std::io::stdout().flush().ok();
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .context("failed to read callback URL from stdin")?;
    let url = line.trim().to_string();
    if url.is_empty() {
        return Err(anyhow!("no callback URL entered"));
    }
    Ok(url)
}

fn open_browser(url: &str) {
    // Use rundll32 rather than `cmd /C start`: the SSO URL contains `&`, which
    // cmd's parser treats as a command separator (spawning bogus commands).
    #[cfg(target_os = "windows")]
    let result = std::process::Command::new("rundll32.exe")
        .arg("url.dll,FileProtocolHandler")
        .arg(url)
        .spawn();
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open").arg(url).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let result = std::process::Command::new("xdg-open").arg(url).spawn();

    if let Err(err) = result {
        println!("[login] could not launch browser automatically: {err}");
    }
}

fn extract_query_param(url: &str, key: &str) -> Option<String> {
    let query = url.split_once('?')?.1;
    for part in query.split('&') {
        let (k, v) = part.split_once('=')?;
        if k == key {
            return Some(urlencoding::decode(v).ok()?.into_owned());
        }
    }
    None
}
