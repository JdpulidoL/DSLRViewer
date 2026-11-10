//! Operation, response, event and property codes from ISO 15740 (PTP)
//! plus the vendor extensions this application knows how to drive.

pub mod op {
    pub const GET_DEVICE_INFO: u16 = 0x1001;
    pub const OPEN_SESSION: u16 = 0x1002;
    pub const CLOSE_SESSION: u16 = 0x1003;
    pub const GET_OBJECT_HANDLES: u16 = 0x1007;
    pub const GET_OBJECT_INFO: u16 = 0x1008;
    pub const GET_OBJECT: u16 = 0x1009;
    pub const GET_THUMB: u16 = 0x100A;
    pub const DELETE_OBJECT: u16 = 0x100B;
    pub const INITIATE_CAPTURE: u16 = 0x100E;
    pub const GET_DEVICE_PROP_DESC: u16 = 0x1014;
    pub const GET_DEVICE_PROP_VALUE: u16 = 0x1015;
    pub const SET_DEVICE_PROP_VALUE: u16 = 0x1016;

    /// Nikon extensions (vendor extension id 0x0A).
    pub mod nikon {
        pub const AF_DRIVE: u16 = 0x90C1;
        pub const START_LIVE_VIEW: u16 = 0x9201;
        pub const END_LIVE_VIEW: u16 = 0x9202;
        pub const GET_LIVE_VIEW_IMG: u16 = 0x9203;
        pub const MF_DRIVE: u16 = 0x9204;
    }

    /// Canon EOS extensions (vendor extension id 0x0B).
    pub mod canon {
        pub const SET_DEVICE_PROP_VALUE_EX: u16 = 0x9110;
        pub const SET_REMOTE_MODE: u16 = 0x9114;
        pub const SET_EVENT_MODE: u16 = 0x9115;
        pub const GET_EVENT: u16 = 0x9116;
        /// Tell the body how much room the PC has when images are sent to it.
        pub const PC_HDD_CAPACITY: u16 = 0x911A;
        pub const GET_PARTIAL_OBJECT: u16 = 0x9107;
        pub const TRANSFER_COMPLETE: u16 = 0x9117;
        pub const REMOTE_RELEASE: u16 = 0x910F;
        pub const REMOTE_RELEASE_ON: u16 = 0x9128;
        pub const REMOTE_RELEASE_OFF: u16 = 0x9129;
        pub const GET_VIEW_FINDER_DATA: u16 = 0x9153;
        pub const DRIVE_LENS: u16 = 0x9155;
    }
}

pub mod rc {
    pub const OK: u16 = 0x2001;
    pub const GENERAL_ERROR: u16 = 0x2002;
    pub const SESSION_NOT_OPEN: u16 = 0x2003;
    pub const OPERATION_NOT_SUPPORTED: u16 = 0x2005;
    pub const PARAMETER_NOT_SUPPORTED: u16 = 0x2006;
    pub const INVALID_OBJECT_HANDLE: u16 = 0x2009;
    pub const DEVICE_PROP_NOT_SUPPORTED: u16 = 0x200A;
    pub const STORE_FULL: u16 = 0x200C;
    pub const DEVICE_BUSY: u16 = 0x2019;
    pub const SESSION_ALREADY_OPEN: u16 = 0x201E;
    /// Nikon: live view is not (yet) running / frame not ready.
    pub const NIKON_NOT_LIVE_VIEW: u16 = 0xA00B;
    /// Canon: live view image not ready yet.
    pub const CANON_NOT_READY: u16 = 0xA102;

    pub fn name(code: u16) -> &'static str {
        match code {
            OK => "OK",
            GENERAL_ERROR => "GeneralError",
            SESSION_NOT_OPEN => "SessionNotOpen",
            OPERATION_NOT_SUPPORTED => "OperationNotSupported",
            PARAMETER_NOT_SUPPORTED => "ParameterNotSupported",
            INVALID_OBJECT_HANDLE => "InvalidObjectHandle",
            DEVICE_PROP_NOT_SUPPORTED => "DevicePropNotSupported",
            STORE_FULL => "StoreFull",
            DEVICE_BUSY => "DeviceBusy",
            SESSION_ALREADY_OPEN => "SessionAlreadyOpen",
            NIKON_NOT_LIVE_VIEW => "NotLiveView",
            CANON_NOT_READY => "NotReady",
            _ => "Unknown",
        }
    }
}

pub mod ev {
    pub const OBJECT_ADDED: u16 = 0x4002;
    pub const DEVICE_PROP_CHANGED: u16 = 0x4006;
    pub const CAPTURE_COMPLETE: u16 = 0x400D;
}

pub mod prop {
    pub const BATTERY_LEVEL: u16 = 0x5001;
    pub const WHITE_BALANCE: u16 = 0x5005;
    pub const F_NUMBER: u16 = 0x5007;
    pub const FOCUS_MODE: u16 = 0x500A;
    pub const METERING_MODE: u16 = 0x500B;
    pub const EXPOSURE_TIME: u16 = 0x500D;
    pub const EXPOSURE_PROGRAM: u16 = 0x500E;
    pub const EXPOSURE_INDEX: u16 = 0x500F;
    pub const EXPOSURE_BIAS: u16 = 0x5010;

    /// Canon EOS electronic viewfinder properties.
    pub const CANON_EVF_OUTPUT_DEVICE: u32 = 0xD1B0;
    pub const CANON_EVF_MODE: u32 = 0xD1B1;
}

/// PTP vendor extension identifiers reported by `GetDeviceInfo`.
pub mod vendor {
    pub const NIKON: u32 = 0x0000_000A;
    pub const CANON: u32 = 0x0000_000B;
}

/// Object format code for EXIF/JPEG images.
pub const FORMAT_EXIF_JPEG: u16 = 0x3801;
