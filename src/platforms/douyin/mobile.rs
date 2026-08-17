//! Signed Douyin mobile device registration and aweme detail fetch.

use std::time::{SystemTime, UNIX_EPOCH};

use reqwest::{
    Client, Method, StatusCode,
    header::{CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue, USER_AGENT},
};
use serde_json::{Value, json};
use url::Url;
use uuid::Uuid;

use crate::{
    Error, Result,
    platforms::util::{map_network_error, read_body_limited},
};

use super::{
    hosts::is_allowed_api_host,
    sign::{SignedHeaders, encode_query, sign_query, trace_id},
};

const MOBILE_USER_AGENT: &str = "com.ss.android.ugc.aweme/390500 (Linux; U; Android 13; zh_CN; Pixel 6; \
    Build/TQ3A.230805.001; Cronet/TTNetVersion:6b6f6e6e 2024-04-10 \
    QuicVersion:47946d2a 2024-03-28)";
const AID: u32 = 1128;
const LICENSE_ID: u32 = 1_611_921_764;
const GORGON_PROFILES: [u16; 2] = [8404, 4404];
const REGISTER_HOSTS: &[&str] = &["log.snssdk.com", "api.amemv.com"];
const DETAIL_HOSTS: &[&str] = &[
    "api.amemv.com",
    "api3-core-c.amemv.com",
    "aweme.snssdk.com",
    "api5-normal-lf.amemv.com",
    "api3-normal-c.amemv.com",
];
const MAX_JSON_BYTES: usize = 1024 * 1024;
const MAX_REGISTER_ATTEMPTS: usize = 3;
const MAX_DETAIL_ROUNDS: usize = 3;

#[derive(Debug, Clone)]
pub struct MobileDevice {
    pub device_id: String,
    pub iid: String,
    pub cdid: String,
    pub openudid: String,
}

impl MobileDevice {
    pub fn from_env() -> Option<Self> {
        let device_id = first_env(&[
            "PARSE_KIT_DOUYIN_DEVICE_ID",
            "PARSEHUB_DOUYIN_DEVICE_ID",
            "DOUYIN_DEVICE_ID",
        ])?;
        let iid = first_env(&[
            "PARSE_KIT_DOUYIN_IID",
            "PARSEHUB_DOUYIN_IID",
            "PARSEHUB_DOUYIN_INSTALL_ID",
            "DOUYIN_IID",
        ])?;
        Some(Self {
            device_id,
            iid,
            cdid: first_env(&[
                "PARSE_KIT_DOUYIN_CDID",
                "PARSEHUB_DOUYIN_CDID",
                "DOUYIN_CDID",
            ])
            .unwrap_or_else(new_cdid),
            openudid: first_env(&[
                "PARSE_KIT_DOUYIN_OPENUDID",
                "PARSEHUB_DOUYIN_OPENUDID",
                "DOUYIN_OPENUDID",
            ])
            .unwrap_or_else(new_openudid),
        })
    }

    fn from_register(payload: &Value, cdid: String, openudid: String) -> Option<Self> {
        let device_id = first_id(payload, &["device_id_str", "device_id"])?;
        let iid = first_id(payload, &["install_id_str", "install_id", "iid"])?;
        Some(Self {
            device_id,
            iid,
            cdid,
            openudid,
        })
    }
}

pub async fn fetch_aweme_detail(
    client: &Client,
    device: &mut Option<MobileDevice>,
    aweme_id: &str,
) -> Result<Value> {
    let mut last_error = Error::MediaUnavailable;
    for _ in 0..MAX_DETAIL_ROUNDS {
        let current = match ensure_device(client, device).await {
            Ok(value) => value,
            Err(error) => {
                last_error = error;
                *device = None;
                continue;
            }
        };
        match request_aweme_detail(client, &current, aweme_id).await {
            Ok(payload) => {
                *device = Some(current);
                return Ok(payload);
            }
            Err(error) => {
                last_error = error;
                *device = None;
            }
        }
    }
    Err(last_error)
}

async fn ensure_device(client: &Client, device: &mut Option<MobileDevice>) -> Result<MobileDevice> {
    if let Some(existing) = device.clone() {
        return Ok(existing);
    }
    if let Some(from_env) = MobileDevice::from_env() {
        *device = Some(from_env.clone());
        return Ok(from_env);
    }
    let registered = register_device(client).await?;
    *device = Some(registered.clone());
    Ok(registered)
}

async fn register_device(client: &Client) -> Result<MobileDevice> {
    let mut last_error = Error::Network("注册抖音移动端设备失败".into());
    for _ in 0..MAX_REGISTER_ATTEMPTS {
        for host in REGISTER_HOSTS {
            let mut params = common_params();
            append_ephemeral_ids(&mut params);
            let query = encode_query(&params);
            let url = api_url(host, "/service/2/device_register/", &query)?;
            let headers = signed_headers(&params, GORGON_PROFILES[0], true);
            let body = register_payload(&params);
            match send_json(client, Method::POST, url, headers, Some(body)).await {
                Ok(payload) => {
                    if let Some(device) = MobileDevice::from_register(
                        &payload,
                        param_value(&params, "cdid"),
                        param_value(&params, "openudid"),
                    ) {
                        tracing::debug!(
                            event = "douyin_device_registered",
                            host,
                            "registered Douyin mobile device"
                        );
                        return Ok(device);
                    }
                    last_error = Error::UpstreamChanged;
                }
                Err(error) => last_error = error,
            }
        }
    }
    Err(last_error)
}

async fn request_aweme_detail(
    client: &Client,
    device: &MobileDevice,
    aweme_id: &str,
) -> Result<Value> {
    let mut params = common_params();
    params.insert(0, ("aweme_id".into(), aweme_id.to_owned()));
    set_param(&mut params, "is_guest_mode", "0");
    set_param(&mut params, "minor_status", "0");
    append_ephemeral_ids(&mut params);
    set_param(&mut params, "device_id", &device.device_id);
    set_param(&mut params, "iid", &device.iid);
    set_param(&mut params, "cdid", &device.cdid);
    set_param(&mut params, "openudid", &device.openudid);
    let query = encode_query(&params);

    let mut last_error = Error::MediaUnavailable;
    for version in GORGON_PROFILES {
        let headers = signed_headers(&params, version, false);
        for host in DETAIL_HOSTS {
            let url = api_url(host, "/aweme/v1/aweme/detail/", &query)?;
            match send_json(client, Method::GET, url, headers.clone(), None).await {
                Ok(payload) => {
                    if payload.get("aweme_detail").is_some() {
                        return Ok(payload);
                    }
                    last_error = classify_detail_error(&payload);
                }
                Err(error) => last_error = error,
            }
        }
    }
    Err(last_error)
}

fn classify_detail_error(payload: &Value) -> Error {
    let status_msg = payload
        .get("status_msg")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let lowered = status_msg.to_ascii_lowercase();
    if lowered.contains("not exist")
        || lowered.contains("not found")
        || status_msg.contains("不存在")
        || status_msg.contains("删除")
    {
        Error::NotFound
    } else if payload.get("status_code").and_then(Value::as_i64) == Some(0) {
        Error::MediaUnavailable
    } else {
        Error::UpstreamChanged
    }
}

fn signed_headers(params: &[(String, String)], gorgon_version: u16, json_body: bool) -> HeaderMap {
    let query = encode_query(params);
    let signed: SignedHeaders = sign_query(&query, AID, LICENSE_ID, gorgon_version);
    let device_id = param_value(params, "device_id");
    let mut headers = HeaderMap::new();
    insert_header(&mut headers, USER_AGENT, MOBILE_USER_AGENT);
    insert_header(&mut headers, "x-ss-req-ticket", &signed.x_ss_req_ticket);
    insert_header(&mut headers, "x-khronos", &signed.x_khronos);
    insert_header(&mut headers, "x-gorgon", &signed.x_gorgon);
    insert_header(&mut headers, "x-ss-stub", &signed.x_ss_stub);
    insert_header(&mut headers, "x-ladon", &signed.x_ladon);
    insert_header(&mut headers, "x-argus", &signed.x_argus);
    insert_header(&mut headers, "x-tt-trace-id", &trace_id(&device_id));
    insert_header(&mut headers, "sdk-version", "2");
    insert_header(&mut headers, "passport-sdk-version", "203226");
    if json_body {
        insert_header(
            &mut headers,
            CONTENT_TYPE,
            "application/json; charset=utf-8",
        );
    }
    headers
}

fn insert_header(headers: &mut HeaderMap, name: impl TryInto<HeaderName>, value: &str) {
    let Ok(name) = name.try_into() else {
        return;
    };
    if let Ok(value) = HeaderValue::from_str(value) {
        headers.insert(name, value);
    }
}

async fn send_json(
    client: &Client,
    method: Method,
    url: Url,
    headers: HeaderMap,
    body: Option<Vec<u8>>,
) -> Result<Value> {
    let mut request = client.request(method, url).headers(headers);
    if let Some(body) = body {
        request = request.body(body);
    }
    let response = request
        .send()
        .await
        .map_err(|error| map_network_error(&error, "抖音请求超时", "抖音网络请求失败"))?;
    let status = response.status();
    if status == StatusCode::TOO_MANY_REQUESTS {
        return Err(Error::RateLimited);
    }
    if status == StatusCode::NOT_FOUND {
        return Err(Error::NotFound);
    }
    if !status.is_success() {
        return Err(Error::Network(format!("抖音接口 HTTP {}", status.as_u16())));
    }
    let bytes = read_body_limited(response, MAX_JSON_BYTES, |error| {
        map_network_error(error, "抖音请求超时", "抖音网络请求失败")
    })
    .await?;
    if bytes.is_empty() {
        return Err(Error::UpstreamChanged);
    }
    serde_json::from_slice(&bytes).map_err(|_| Error::UpstreamChanged)
}

fn api_url(host: &str, path: &str, query: &str) -> Result<Url> {
    if !is_allowed_api_host(host) {
        return Err(Error::Network("抖音接口主机未允许".into()));
    }
    Url::parse(&format!("https://{host}{path}?{query}")).map_err(|_| Error::UpstreamChanged)
}

fn common_params() -> Vec<(String, String)> {
    [
        ("aid", "1128"),
        ("app_name", "aweme"),
        ("version_code", "390500"),
        ("version_name", "39.5.0"),
        ("device_platform", "android"),
        ("os", "android"),
        ("os_version", "13"),
        ("ssmix", "a"),
        ("language", "zh"),
        ("channel", "wandoujia_aweme"),
        ("device_type", "Pixel 6"),
        ("device_brand", "google"),
        ("resolution", "1080*2400"),
        ("dpi", "420"),
        ("host_abi", "arm64-v8a"),
        ("manifest_version_code", "390500"),
        ("update_version_code", "390500"),
        ("ac", "wifi"),
        ("app_type", "normal"),
        ("cpu_support64", "true"),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_owned(), value.to_owned()))
    .collect()
}

fn append_ephemeral_ids(params: &mut Vec<(String, String)>) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0);
    set_param(params, "_rticket", &millis.to_string());
    set_param(params, "cdid", &new_cdid());
    set_param(params, "ts", &now.to_string());
    set_param(params, "iid", &random_decimal_id());
    set_param(params, "device_id", &random_decimal_id());
    set_param(params, "openudid", &new_openudid());
}

fn register_payload(params: &[(String, String)]) -> Vec<u8> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let payload = json!({
        "magic_tag": "ss_app_log",
        "header": {
            "display_name": "抖音",
            "aid": 1128,
            "channel": "wandoujia_aweme",
            "package": "com.ss.android.ugc.aweme",
            "app_version": "39.5.0",
            "version_code": 390500,
            "manifest_version_code": 390500,
            "update_version_code": 390500,
            "sdk_version": "3.9.5",
            "sdk_target_version": 29,
            "os": "Android",
            "os_version": "13",
            "os_api": 33,
            "device_model": "Pixel 6",
            "device_brand": "google",
            "device_manufacturer": "Google",
            "cpu_abi": "arm64-v8a",
            "release_build": "TQ3A.230805.001",
            "density_dpi": 420,
            "display_density": "xhdpi",
            "resolution": "1080x2400",
            "language": "zh",
            "timezone": 8,
            "region": "CN",
            "tz_name": "Asia/Shanghai",
            "cdid": param_value(params, "cdid"),
            "openudid": param_value(params, "openudid"),
            "clientudid": Uuid::new_v4().to_string(),
            "google_aid": "",
            "req_id": Uuid::new_v4().to_string(),
        },
        "_gen_time": now,
    });
    serde_json::to_vec(&payload).unwrap_or_default()
}

fn set_param(params: &mut Vec<(String, String)>, key: &str, value: &str) {
    if let Some(entry) = params.iter_mut().find(|(existing, _)| existing == key) {
        entry.1 = value.to_owned();
    } else {
        params.push((key.to_owned(), value.to_owned()));
    }
}

fn param_value(params: &[(String, String)], key: &str) -> String {
    params
        .iter()
        .find(|(existing, _)| existing == key)
        .map(|(_, value)| value.clone())
        .unwrap_or_default()
}

fn first_env(keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        std::env::var(key)
            .ok()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
    })
}

fn first_id(payload: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        payload.get(*key).and_then(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .or_else(|| value.as_u64().map(|number| number.to_string()))
                .or_else(|| value.as_i64().map(|number| number.to_string()))
                .filter(|id| !id.is_empty() && id != "0")
        })
    })
}

fn new_cdid() -> String {
    Uuid::new_v4().to_string()
}

fn new_openudid() -> String {
    hex_of(&Uuid::new_v4().as_bytes()[..8])
}

fn random_decimal_id() -> String {
    let bytes = Uuid::new_v4();
    let value = u128::from_be_bytes(*bytes.as_bytes()) % 10u128.pow(19);
    value.max(1).to_string()
}

fn hex_of(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}
