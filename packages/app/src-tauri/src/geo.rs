//! 逆地理编码：把 EXIF 里的 GPS 坐标（WGS-84）转成一行简略地名。
//!
//! 走 Rust 侧请求而非前端 fetch：amap/baidu 的接口不带 CORS 头，
//! WebView 的自定义 origin 下直接 fetch 会被拦。
//! 三个来源都是免费接口：
//! - Nominatim（OpenStreetMap）：免 Key，限约 1 次/秒，仅打开详情抽屉时触发；
//! - 高德：个人 Key（Web 服务类型），入参要求 GCJ-02，本地偏移转换（纯数学）；
//! - 百度：个人 AK，coordtype=wgs84ll 直接收 WGS-84。

use std::sync::OnceLock;
use std::time::Duration;

use serde_json::Value;

/// 5 秒超时：地图接口偶尔慢，但不能拖住详情抽屉太久。
const TIMEOUT: Duration = Duration::from_secs(5);
/// Nominatim 使用政策要求请求带可识别的 UA（默认 UA 会被拒绝）。
const USER_AGENT: &str = concat!("sviewer/", env!("CARGO_PKG_VERSION"), " (github.com/while-coder/sviewer)");

fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(TIMEOUT)
            .user_agent(USER_AGENT)
            .build()
            .expect("构建 HTTP 客户端失败")
    })
}

/// 逆地理编码入口。返回简略地名（如「北京市海淀区中关村大街 1 号」），查不到返回 None。
/// provider：osm / amap / baidu；amap、baidu 需要对应 Key，缺 Key 时报错提示。
pub async fn reverse_geocode(
    lat: f64,
    lng: f64,
    provider: &str,
    amap_key: Option<&str>,
    baidu_key: Option<&str>,
) -> Result<Option<String>, String> {
    match provider {
        "osm" => osm_reverse(lat, lng).await,
        "amap" => {
            let key = amap_key.filter(|k| !k.trim().is_empty()).ok_or("未配置高德 Key")?;
            amap_reverse(lat, lng, key.trim()).await
        }
        "baidu" => {
            let key = baidu_key.filter(|k| !k.trim().is_empty()).ok_or("未配置百度 AK")?;
            baidu_reverse(lat, lng, key.trim()).await
        }
        _ => Ok(None),
    }
}

/// Nominatim 逆地理：display_name 即完整地址；zoom=16 街道级，配合 accept-language=zh 出中文。
async fn osm_reverse(lat: f64, lng: f64) -> Result<Option<String>, String> {
    let url = format!(
        "https://nominatim.openstreetmap.org/reverse?format=jsonv2&lat={lat}&lon={lng}&zoom=16&accept-language=zh-CN,zh"
    );
    let v: Value = client()
        .get(&url)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| format!("Nominatim 请求失败：{e}"))?
        .json()
        .await
        .map_err(|e| format!("Nominatim 响应解析失败：{e}"))?;
    Ok(v["display_name"].as_str().map(str::trim).filter(|s| !s.is_empty()).map(Into::into))
}

/// 高德逆地理。入参只认 GCJ-02，EXIF 的 WGS-84 先做本地偏移转换。
/// 响应：{ status: "1", regeocode: { formatted_address } }。
async fn amap_reverse(lat: f64, lng: f64, key: &str) -> Result<Option<String>, String> {
    let (glat, glng) = wgs84_to_gcj02(lat, lng);
    let url = format!(
        "https://restapi.amap.com/v3/geocode/regeo?key={key}&location={glng:.6},{glat:.6}"
    );
    let v: Value = client()
        .get(&url)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| format!("高德请求失败：{e}"))?
        .json()
        .await
        .map_err(|e| format!("高德响应解析失败：{e}"))?;
    // status 非 "1"（Key 无效 / 配额超限等）时把 info 带出来方便排查
    if v["status"].as_str() != Some("1") {
        let info = v["info"].as_str().unwrap_or("未知错误");
        return Err(format!("高德接口返回错误：{info}"));
    }
    let addr = v["regeocode"]["formatted_address"].as_str().map(str::trim);
    Ok(addr.filter(|s| !s.is_empty()).map(Into::into))
}

/// 百度逆地理。coordtype=wgs84ll 直接收 WGS-84，免转换。
/// 响应：{ status: 0, result: { formatted_address } }。
async fn baidu_reverse(lat: f64, lng: f64, key: &str) -> Result<Option<String>, String> {
    let url = format!(
        "https://api.map.baidu.com/reverse_geocoding/v3/?ak={key}&output=json&coordtype=wgs84ll&location={lat:.6},{lng:.6}"
    );
    let v: Value = client()
        .get(&url)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| format!("百度请求失败：{e}"))?
        .json()
        .await
        .map_err(|e| format!("百度响应解析失败：{e}"))?;
    if v["status"].as_i64() != Some(0) {
        let msg = v["msg"].as_str().unwrap_or("未知错误");
        return Err(format!("百度接口返回错误：{msg}"));
    }
    let addr = v["result"]["formatted_address"].as_str().map(str::trim);
    Ok(addr.filter(|s| !s.is_empty()).map(Into::into))
}

// ── WGS-84 → GCJ-02 偏移转换（公开的标准纠偏算法，误差约 1~2 米）──

const PI: f64 = std::f64::consts::PI;
/// 克拉索夫斯基椭球长半轴（米）
const SEMI_MAJOR: f64 = 6378245.0;
/// 第一偏心率的平方
const ECC_SQ: f64 = 0.006_693_421_622_965_943;

/// 粗判是否在中国大陆坐标范围外：境外高德坐标系与 WGS-84 一致，不做偏移。
fn out_of_china(lat: f64, lng: f64) -> bool {
    lng < 72.004 || lng > 137.8347 || lat < 0.8293 || lat > 55.8271
}

fn transform_lat(x: f64, y: f64) -> f64 {
    let mut ret =
        -100.0 + 2.0 * x + 3.0 * y + 0.2 * y * y + 0.1 * x * y + 0.2 * x.abs().sqrt();
    ret += (20.0 * (6.0 * x * PI).sin() + 20.0 * (2.0 * x * PI).sin()) * 2.0 / 3.0;
    ret += (20.0 * (y * PI).sin() + 40.0 * (y / 3.0 * PI).sin()) * 2.0 / 3.0;
    ret += (160.0 * (y / 12.0 * PI).sin() + 320.0 * (y * PI / 30.0).sin()) * 2.0 / 3.0;
    ret
}

fn transform_lng(x: f64, y: f64) -> f64 {
    let mut ret =
        300.0 + x + 2.0 * y + 0.1 * x * x + 0.1 * x * y + 0.1 * x.abs().sqrt();
    ret += (20.0 * (6.0 * x * PI).sin() + 20.0 * (2.0 * x * PI).sin()) * 2.0 / 3.0;
    ret += (20.0 * (x * PI).sin() + 40.0 * (x / 3.0 * PI).sin()) * 2.0 / 3.0;
    ret += (150.0 * (x / 12.0 * PI).sin() + 300.0 * (x / 30.0 * PI).sin()) * 2.0 / 3.0;
    ret
}

/// WGS-84 经纬度转 GCJ-02（火星坐标系）。
pub fn wgs84_to_gcj02(lat: f64, lng: f64) -> (f64, f64) {
    if out_of_china(lat, lng) {
        return (lat, lng);
    }
    let dlat = transform_lat(lng - 105.0, lat - 35.0);
    let dlng = transform_lng(lng - 105.0, lat - 35.0);
    let radlat = lat / 180.0 * PI;
    let magic = 1.0 - ECC_SQ * radlat.sin().powi(2);
    let sqrtmagic = magic.sqrt();
    let dlat = (dlat * 180.0) / ((SEMI_MAJOR * (1.0 - ECC_SQ)) / (magic * sqrtmagic) * PI);
    let dlng = (dlng * 180.0) / (SEMI_MAJOR / sqrtmagic * radlat.cos() * PI);
    (lat + dlat, lng + dlng)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 天安门附近（WGS-84 约 39.9075, 116.3913）：偏移量应在数百米量级（<0.02°），
    /// 且经度增加（中国境内 GCJ-02 相对 WGS-84 整体东偏）。
    #[test]
    fn wgs84_to_gcj02_beijing_offset() {
        let (lat, lng) = wgs84_to_gcj02(39.9075, 116.3913);
        let (dlat, dlng) = (lat - 39.9075, lng - 116.3913);
        assert!(dlat.abs() < 0.02 && dlng.abs() < 0.02, "偏移异常：({dlat}, {dlng})");
        assert!(dlat > 0.0 && dlng > 0.0, "境内偏移方向应为东北：({dlat}, {dlng})");
    }

    /// 境外坐标（纽约）不做偏移，原样返回。
    #[test]
    fn wgs84_to_gcj02_overseas_untouched() {
        assert_eq!(wgs84_to_gcj02(40.7128, -74.0060), (40.7128, -74.0060));
    }
}
