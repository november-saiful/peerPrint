use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const PROTO_VERSION: u16 = 1;
pub const ALPN: &[u8] = b"peerprint/1";
pub const MAX_FRAME: usize = 1024 * 1024;
pub const CHUNK_SIZE: usize = 32 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PrinterMeta {
    pub id: Uuid,
    pub name: String,
    pub model: String,
    pub driver_name: String,
    pub driver_env: String,
    pub driver_version: u64,
    pub online: bool,
    pub status: String,
    pub jobs: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PrinterStatusInfo {
    pub id: Uuid,
    pub online: bool,
    pub status: String,
    pub jobs: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ClientMsg {
    Hello {
        proto: u16,
        app_version: String,
    },
    Info,
    PairInit {
        spake_a: Vec<u8>,
        client_name: String,
    },
    PairFinish {
        confirm_a: [u8; 32],
        client_id: Uuid,
    },
    Auth {
        client_id: Uuid,
        token: [u8; 32],
    },
    Status,
    JobStart {
        tx: u32,
        printer_id: Uuid,
        client_job_ref: Uuid,
        doc_name: String,
        len: u64,
        sha256: [u8; 32],
    },
    JobData {
        tx: u32,
        data: Vec<u8>,
    },
    JobEnd {
        tx: u32,
    },
    JobCancel {
        tx: u32,
        reason: String,
    },
    Ping {
        nonce: u64,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum HostMsg {
    HelloAck {
        proto: u16,
        host_id: Uuid,
        display_name: String,
        pin_required: bool,
        tls_fp: [u8; 32],
    },
    InfoReply {
        host_id: Uuid,
        display_name: String,
        printers: Vec<PrinterMeta>,
    },
    PairChallenge {
        spake_b: Vec<u8>,
    },
    PairOk {
        confirm_b: [u8; 32],
        client_id: Uuid,
        host_id: Uuid,
        token: [u8; 32],
        display_name: String,
        printers: Vec<PrinterMeta>,
    },
    AuthOk {
        display_name: String,
        printers: Vec<PrinterMeta>,
    },
    StatusReply {
        printers: Vec<PrinterStatusInfo>,
    },
    JobAccepted {
        tx: u32,
        job_id: u32,
    },
    JobProgress {
        tx: u32,
        pages_printed: u32,
        total_pages: u32,
    },
    JobDone {
        tx: u32,
        pages: u32,
    },
    JobFailed {
        tx: u32,
        code: ErrCode,
        message: String,
        retryable: bool,
    },
    Pong {
        nonce: u64,
    },
    Error {
        code: ErrCode,
        message: String,
        fatal: bool,
    },
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum ErrCode {
    BadPin,
    LockedOut,
    Revoked,
    Cancelled,
    Unauthorized,
    UnknownPrinter,
    Busy,
    Protocol,
    FrameTooLarge,
    RateLimited,
    PrinterOffline,
    DriverMissing,
    AlreadyInstalled,
    Internal,
}
