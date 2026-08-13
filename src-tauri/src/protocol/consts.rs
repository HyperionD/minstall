//! 协议常量：唯一来源 docs/protocol-notes.md（POC 真机验证值）。改协议只改本文件。
//!
//! Band 10 Pro 的 V2 协议走经典蓝牙 SPP 通道（RFCOMM ch5），见协议笔记 4.3/4.5 节；
//! 表盘安装用 astrobox 的 WearPacket 协议，见协议笔记 5 节。所有值均有真机或参考来源，禁止臆造。

/// 标准 Serial Port Profile UUID（协议笔记 4.5 节，真机确认）
pub const SPP_SERVICE_UUID: &str = "00001101-0000-1000-8000-00805f9b34fb";
/// RFCOMM 通道号（协议笔记 4.3/4.5 节，真机确认 = 5）
pub const RFCOMM_CHANNEL: u8 = 5;
/// V1 Hello 帧（协议笔记 4.3 节：SPP 连接后必须先行发送，手环回版本帧确认通道）
pub const V1_HELLO: &[u8] = &[0xba, 0xdc, 0xfe, 0x00, 0xc0, 0x03, 0x00, 0x00, 0x01, 0x00, 0xef];

// ---- V2 帧格式（协议笔记 4 节）----
/// 帧头 preamble（2 字节）
pub const V2_PREAMBLE: [u8; 2] = [0xa5, 0xa5];
/// packet type（低 nibble）：ACK
pub const V2_PACKET_ACK: u8 = 1;
/// packet type：SESSION_CONFIG
pub const V2_PACKET_SESSION_CONFIG: u8 = 2;
/// packet type：DATA
pub const V2_PACKET_DATA: u8 = 3;
/// 帧头长度：preamble 2 + type 1 + seq 1 + payload_len 2 + crc 2
pub const V2_HEADER_LEN: usize = 8;

// ---- DATA 包 payload（协议笔记 4 节）----
/// channel（低 nibble）：PROTOBUF（加密）
pub const CHANNEL_PROTOBUF: u8 = 1;
/// channel：DATA（明文）
pub const CHANNEL_DATA: u8 = 2;
/// channel：ACTIVITY（加密）
pub const CHANNEL_ACTIVITY: u8 = 5;
/// opCode：PLAINTEXT
pub const OPCODE_PLAINTEXT: u8 = 1;
/// opCode：ENCRYPTED
pub const OPCODE_ENCRYPTED: u8 = 2;

// ---- authkey（协议笔记 4 节，真机确认）----
/// authkey hex 字符数（16 字节）
pub const AUTHKEY_LEN: usize = 32;

// ---- WearPacket 认证（协议笔记 4.4 节，astrobox 同款）----
/// id=AUTH_VERIFY
pub const AUTH_ID_VERIFY: u8 = 26;
/// id=AUTH_CONFIRM
pub const AUTH_ID_CONFIRM: u8 = 27;
/// 错误码 NO_BOUND（手环未绑定 → authkey 无效）
pub const AUTH_ERROR_NO_BOUND: u8 = 4;

// ---- WearPacket 表盘安装（协议笔记 5 节）----
/// type=ACCOUNT
pub const WEARPACKET_TYPE_ACCOUNT: u8 = 1;
/// type=SYSTEM
pub const WEARPACKET_TYPE_SYSTEM: u8 = 2;
/// type=WATCH_FACE
pub const WEARPACKET_TYPE_WATCH_FACE: u8 = 4;
/// type=MASS（注意：WearPacket.type=22，而 payload oneof 字段 Mass=24 —— 两个不同概念，见 WEARPACKET_PAYLOAD_MASS）
pub const WEARPACKET_TYPE_MASS: u8 = 22;
/// WearPacket payload oneof 中 Mass 的字段号 = 24（非 7！字段编号见协议笔记 5 节）
pub const WEARPACKET_PAYLOAD_MASS: u8 = 24;

/// id=GET_INSTALLED_LIST
pub const WP_ID_GET_INSTALLED_LIST: u8 = 0;
/// id=PREPARE_INSTALL_WATCH_FACE
pub const WP_ID_PREPARE_INSTALL_WATCH_FACE: u8 = 4;
/// id=REPORT_INSTALL_RESULT
pub const WP_ID_REPORT_INSTALL_RESULT: u8 = 5;
/// id=GET_STORAGE_INFO
pub const WP_ID_GET_STORAGE_INFO: u8 = 62;
/// Mass id=PREPARE
pub const WP_ID_MASS_PREPARE: u8 = 0;

// ---- MASS 分片上传（协议笔记 5 节，真机确认）----
/// MassPacket data_type（表盘数据）
pub const MASS_DATA_TYPE: u8 = 16;
/// MASS 帧头开销：L2 头 2B + total u16 + cur u16（4B）= 6B；fragment = slice_length - 6
pub const MASS_FRAME_OVERHEAD: usize = 6;
/// 默认分片长度：协议上由设备 prepare_response.expected_slice_length 给出（真机返回 12288），此处为初始值，运行时以设备返回为准
pub const DEFAULT_SLICE_LENGTH: usize = 12288;
/// 批量窗口：真机验证 BATCH=2 稳定但慢（140s/2.4MB）；
/// 大 BATCH（18）快（~30s）且不丢数据（逐批等 ACK），但手环可能不推 InstallResult，
/// 需依赖表盘列表查询兑底确认。默认 18（可用 MINSTALL_MASS_BATCH 覆盖）。
pub const MASS_BATCH: usize = 18;

// ---- 安装结果码（协议笔记 5 节）----
/// code=2：SUCCESS
pub const INSTALL_RESULT_SUCCESS: u8 = 2;
/// code=3：INSTALL_USED（已安装，成功）
pub const INSTALL_RESULT_USED: u8 = 3;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_notes_values_are_non_empty() {
        assert!(!SPP_SERVICE_UUID.is_empty());
        assert!(RFCOMM_CHANNEL > 0);
        assert_eq!(AUTHKEY_LEN, 32);
        assert!(!V1_HELLO.is_empty());
    }

    #[test]
    fn wearpacket_mass_type_and_payload_field() {
        // 协议笔记 5 节明确：type=MASS(22)，payload oneof 字段 Mass=24（不是 7），防混用
        assert_eq!(WEARPACKET_TYPE_MASS, 22);
        assert_eq!(WEARPACKET_PAYLOAD_MASS, 24);
    }

    #[test]
    fn fragment_size_math() {
        // fragment = slice_length - 6
        assert!(DEFAULT_SLICE_LENGTH > MASS_FRAME_OVERHEAD);
    }

    #[test]
    fn v2_header_len_matches_layout() {
        // preamble(2) + type(1) + seq(1) + payload_len(2) + crc(2)
        assert_eq!(V2_HEADER_LEN, 8);
    }
}
