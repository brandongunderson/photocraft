//! Opt-in persistent native-worker transport. Bulk data remains in host-owned files.
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{ApiError, ErrorCode, Result, identifier, invalid, json_size};

pub const WORKER_PROTOCOL_VERSION: u32 = 2;
pub const MAX_WORKER_DESCRIPTOR_BYTES: usize = 16 * 1024;
pub const MAX_WORKER_FRAME_BYTES: usize = 256 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub enum WorkerExecutionProfile {
    TrustedNative,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub enum WorkerOperation {
    Image,
    Task,
}

/// Admission estimates, not operating-system enforcement of a trusted native process.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkerResources {
    #[cfg_attr(feature = "schema", schemars(range(min = 1, max = 64)))]
    pub cpu_units: u32,
    #[cfg_attr(feature = "schema", schemars(range(min = 1, max = 68_719_476_736_u64)))]
    pub memory_bytes: u64,
    #[cfg_attr(feature = "schema", schemars(range(max = 68_719_476_736_u64)))]
    pub gpu_bytes: u64,
    #[cfg_attr(feature = "schema", schemars(range(max = 274_877_906_944_u64)))]
    pub temp_bytes: u64,
    #[cfg_attr(feature = "schema", schemars(range(max = 1_099_511_627_776_u64)))]
    pub cache_bytes: u64,
}

impl WorkerResources {
    pub fn validate(&self) -> Result<()> {
        if !(1..=64).contains(&self.cpu_units)
            || !(1..=64 * 1024 * 1024 * 1024).contains(&self.memory_bytes)
            || self.gpu_bytes > 64 * 1024 * 1024 * 1024
            || self.temp_bytes > 256 * 1024 * 1024 * 1024
            || self.cache_bytes > 1024 * 1024 * 1024 * 1024
        {
            return Err(invalid("worker.resources", "native worker resource declaration exceeds supported bounds"));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkerV2Descriptor {
    #[cfg_attr(feature = "schema", schemars(range(min = 2, max = 2)))]
    pub protocol_version: u32,
    pub execution_profile: WorkerExecutionProfile,
    #[cfg_attr(feature = "schema", schemars(length(min = 1, max = 2)))]
    pub operations: Vec<WorkerOperation>,
    #[serde(default = "idle_timeout")]
    #[cfg_attr(feature = "schema", schemars(range(min = 1000, max = 300_000)))]
    pub idle_timeout_ms: u64,
    #[serde(default = "request_timeout")]
    #[cfg_attr(feature = "schema", schemars(range(min = 100, max = 2_700_000)))]
    pub request_timeout_ms: u64,
    pub resources: WorkerResources,
}

fn idle_timeout() -> u64 {
    60_000
}

fn request_timeout() -> u64 {
    2_700_000
}

impl WorkerV2Descriptor {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > MAX_WORKER_DESCRIPTOR_BYTES {
            return Err(ApiError::new(ErrorCode::LimitExceeded, "worker declaration exceeds 16 KiB"));
        }
        let declaration: Self = serde_json::from_slice(bytes).map_err(|error| ApiError::new(ErrorCode::InvalidJson, error.to_string()))?;
        declaration.validate()?;
        Ok(declaration)
    }

    pub fn validate(&self) -> Result<()> {
        if self.protocol_version != WORKER_PROTOCOL_VERSION {
            return Err(ApiError::new(ErrorCode::IncompatibleApi, "native worker protocol must be version 2"));
        }
        if self.operations.is_empty() || self.operations.len() > 2 || (self.operations.len() == 2 && self.operations.first() == self.operations.get(1)) {
            return Err(invalid("worker.operations", "declare image and/or task without duplicate operations"));
        }
        if !(1000..=300_000).contains(&self.idle_timeout_ms) || !(100..=2_700_000).contains(&self.request_timeout_ms) {
            return Err(invalid("worker.timeouts", "worker idle/request timeout is outside the supported range"));
        }
        self.resources.validate()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase", deny_unknown_fields)]
pub enum HostWorkerFrame {
    Hello {
        id: String,
        protocol_version: u32,
        plugin_id: String,
        generation: String,
        operations: Vec<WorkerOperation>,
        max_frame_bytes: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        managed_storage: Option<Value>,
    },
    Run {
        id: String,
        operation: WorkerOperation,
        request: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        managed_storage: Option<Value>,
    },
    Cancel {
        id: String,
    },
    Reset {
        id: String,
    },
    Shutdown,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase", deny_unknown_fields)]
pub enum WorkerHostFrame {
    Ready { id: String, protocol_version: u32, plugin_id: String, generation: String, operations: Vec<WorkerOperation> },
    Progress { id: String, fraction: f64, message: String },
    Result { id: String },
    Error { id: String, error: ApiError },
    Cancelled { id: String },
    ResetComplete { id: String },
}

fn correlation(id: &str, handshake: bool) -> bool {
    id.len() <= 20 && id.parse::<u64>().is_ok_and(|number| id == number.to_string() && (number != 0 || handshake))
}

impl HostWorkerFrame {
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Hello { id, protocol_version, plugin_id, generation, operations, max_frame_bytes, managed_storage } => {
                if id != "0"
                    || *protocol_version != WORKER_PROTOCOL_VERSION
                    || !identifier(plugin_id, 64)
                    || generation.is_empty()
                    || generation.len() > 128
                    || !generation.bytes().all(|byte| byte.is_ascii_graphic())
                    || operations.is_empty()
                    || operations.len() > 2
                    || (operations.len() == 2 && operations.first() == operations.get(1))
                    || *max_frame_bytes != MAX_WORKER_FRAME_BYTES as u32
                {
                    return Err(invalid("worker.hello", "invalid worker handshake"));
                }
                storage(managed_storage)?;
            }
            Self::Run { id, request, managed_storage, .. } => {
                if !correlation(id, false) || !request.is_object() {
                    return Err(invalid("worker.run", "run requires a nonzero correlation ID and object request"));
                }
                storage(managed_storage)?;
            }
            Self::Cancel { id } | Self::Reset { id } if !correlation(id, false) => return Err(invalid("worker.id", "invalid worker correlation ID")),
            _ => {}
        }
        let value = serde_json::to_value(self).map_err(|error| ApiError::new(ErrorCode::InvalidJson, error.to_string()))?;
        json_size(&value, MAX_WORKER_FRAME_BYTES, "worker.frame")
    }
}

impl WorkerHostFrame {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.is_empty() || bytes.len() > MAX_WORKER_FRAME_BYTES {
            return Err(ApiError::new(ErrorCode::LimitExceeded, "worker frame size must be 1..=256 KiB"));
        }
        let frame: Self = serde_json::from_slice(bytes).map_err(|error| ApiError::new(ErrorCode::InvalidJson, error.to_string()))?;
        frame.validate()?;
        Ok(frame)
    }

    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Ready { id, protocol_version, plugin_id, generation, operations } => {
                HostWorkerFrame::Hello {
                    id: id.clone(),
                    protocol_version: *protocol_version,
                    plugin_id: plugin_id.clone(),
                    generation: generation.clone(),
                    operations: operations.clone(),
                    max_frame_bytes: MAX_WORKER_FRAME_BYTES as u32,
                    managed_storage: None,
                }
                .validate()?;
            }
            Self::Progress { id, fraction, message } => {
                if !correlation(id, false) || !fraction.is_finite() || !(0.0..=1.0).contains(fraction) || message.len() > 1024 {
                    return Err(invalid("worker.progress", "invalid progress ID, fraction or message"));
                }
            }
            Self::Error { id, error } => {
                if !correlation(id, false) || error.message.trim().is_empty() || error.message.len() > 8192 {
                    return Err(invalid("worker.error", "invalid native error ID or message"));
                }
                json_size(&error.details, 16 * 1024, "worker.error.details")?;
            }
            Self::Result { id } | Self::Cancelled { id } | Self::ResetComplete { id } if !correlation(id, false) => {
                return Err(invalid("worker.id", "invalid worker correlation ID"));
            }
            _ => {}
        }
        Ok(())
    }

    pub fn id(&self) -> &str {
        match self {
            Self::Ready { id, .. }
            | Self::Progress { id, .. }
            | Self::Result { id }
            | Self::Error { id, .. }
            | Self::Cancelled { id }
            | Self::ResetComplete { id } => id,
        }
    }
}

fn storage(value: &Option<Value>) -> Result<()> {
    if let Some(value) = value {
        if !value.is_object() {
            return Err(invalid("worker.managedStorage", "managed storage metadata must be an object"));
        }
        json_size(value, 32 * 1024, "worker.managedStorage")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn descriptor() -> Value {
        json!({"protocolVersion":2,"executionProfile":"trustedNative","operations":["image","task"],
            "resources":{"cpuUnits":1,"memoryBytes":1048576,"gpuBytes":0,"tempBytes":0,"cacheBytes":0}})
    }

    #[test]
    fn worker_declaration_is_strict_bounded_and_independently_versioned() {
        let parsed = WorkerV2Descriptor::parse(&serde_json::to_vec(&descriptor()).unwrap()).unwrap();
        assert_eq!(parsed.idle_timeout_ms, 60_000);
        assert_eq!(parsed.request_timeout_ms, 2_700_000);
        assert_eq!(WorkerV2Descriptor::parse(&serde_json::to_vec(&parsed).unwrap()).unwrap(), parsed);
        for (pointer, invalid) in [
            ("/protocolVersion", json!(1)),
            ("/executionProfile", json!("restricted")),
            ("/operations", json!([])),
            ("/operations", json!(["task", "task"])),
            ("/operations", json!(["unknown"])),
            ("/resources/cpuUnits", json!(0)),
            ("/resources/memoryBytes", json!(0)),
            ("/resources/gpuBytes", json!(u64::MAX)),
        ] {
            let mut value = descriptor();
            *value.pointer_mut(pointer).unwrap() = invalid;
            assert!(WorkerV2Descriptor::parse(&serde_json::to_vec(&value).unwrap()).is_err(), "{pointer}");
        }
        let mut unknown = descriptor();
        unknown["installScript"] = json!("run.exe");
        assert!(WorkerV2Descriptor::parse(&serde_json::to_vec(&unknown).unwrap()).is_err());
        assert!(WorkerV2Descriptor::parse(&vec![b' '; MAX_WORKER_DESCRIPTOR_BYTES + 1]).is_err());
    }

    #[test]
    fn worker_frames_reject_bad_ids_shapes_nonfinite_progress_and_oversized_details() {
        for bad in [
            json!({"type":"result","id":"0"}),
            json!({"type":"result","id":"01"}),
            json!({"type":"result","id":"18446744073709551616"}),
            json!({"type":"result","id":"1","extra":true}),
            json!({"type":"progress","id":"1","fraction":1.1,"message":"bad"}),
            json!({"type":"progress","id":"1","fraction":0.5,"message":"x".repeat(1025)}),
            json!({"type":"error","id":"1","error":{"code":"conflict","message":"failure","details":{"large":"x".repeat(16384)}}}),
        ] {
            assert!(WorkerHostFrame::parse(&serde_json::to_vec(&bad).unwrap()).is_err(), "{bad:.80}");
        }
        assert!(WorkerHostFrame::Progress { id: "1".into(), fraction: f64::NAN, message: String::new() }.validate().is_err());
        assert!(WorkerHostFrame::parse(&[0xff]).is_err());
        assert!(WorkerHostFrame::parse(&vec![b' '; MAX_WORKER_FRAME_BYTES + 1]).is_err());
    }

    #[test]
    fn worker_handshake_and_managed_storage_round_trip_without_host_types() {
        let frame = HostWorkerFrame::Hello {
            id: "0".into(),
            protocol_version: 2,
            plugin_id: "org.example.worker".into(),
            generation: "abc:1".into(),
            operations: vec![WorkerOperation::Task],
            max_frame_bytes: MAX_WORKER_FRAME_BYTES as u32,
            managed_storage: Some(json!({"schemaVersion":1,"data":"/private/data"})),
        };
        frame.validate().unwrap();
        assert_eq!(serde_json::from_slice::<HostWorkerFrame>(&serde_json::to_vec(&frame).unwrap()).unwrap(), frame);
        let ready = WorkerHostFrame::parse(
            br#"{"type":"ready","id":"0","protocolVersion":2,"pluginId":"org.example.worker","generation":"abc:1","operations":["task"]}"#,
        )
        .unwrap();
        assert_eq!(ready.id(), "0");
        assert!(
            HostWorkerFrame::Run { id: "1".into(), operation: WorkerOperation::Task, request: json!({}), managed_storage: Some(json!([])) }.validate().is_err()
        );
    }
}
