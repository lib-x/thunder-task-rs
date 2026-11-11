//! Authorized device discovery across gateway and account endpoints.

use reqwest::Method;
use serde_json::Value;

use crate::auth::decode;
use crate::types::Device;
use crate::{Client, Error, Result, DRIVE_PREFIX};

impl Client {
    /// List devices using the drive gateway, falling back to the auth API.
    pub async fn devices(&mut self) -> Result<Vec<Device>> {
        match self
            .authed_request(Method::GET, &format!("{DRIVE_PREFIX}/devices"), None)
            .await
            .and_then(parse_devices)
        {
            Ok(devices) => Ok(devices),
            Err(Error::NotAuthenticated) => Err(Error::NotAuthenticated),
            Err(_) => {
                let url = self.authorized_devices_url()?;
                parse_devices(self.request_url(Method::GET, url, None).await?)
            }
        }
    }
}

fn parse_devices(mut body: Value) -> Result<Vec<Device>> {
    if let Some(data) = body.get_mut("data") {
        body = data.take();
    }
    if body.is_object() {
        body = body
            .get_mut("devices")
            .map(Value::take)
            .or_else(|| body.get_mut("list").map(Value::take))
            .ok_or_else(|| Error::UnexpectedResponse("missing devices/list array".into()))?;
    }
    let mut devices: Vec<Device> = decode(body)?;
    for device in &mut devices {
        if device.space.is_empty() {
            device.space.clone_from(&device.id);
        }
    }
    Ok(devices)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn device_shapes_and_routing() {
        for body in [
            json!([{"id": "nas"}]),
            json!({"devices": [{"device_id": "nas"}]}),
            json!({"list": [{"id": "nas"}]}),
            json!({"data": {"devices": [{"id": "nas"}]}}),
        ] {
            let devices = parse_devices(body).unwrap();
            assert_eq!(devices[0].space, "nas");
        }
        let devices = parse_devices(json!([{"id": "nas", "device_space": "route"}])).unwrap();
        assert_eq!(devices[0].space, "route");
        assert!(parse_devices(json!({"unexpected": []})).is_err());
    }
}
