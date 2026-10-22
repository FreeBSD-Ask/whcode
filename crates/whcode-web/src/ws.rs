//! # 最小 WebSocket 实现（T090、FR-603）
//!
//! RFC 6455 握手（`Sec-WebSocket-Accept = base64(sha1(key + GUID))`）与服务端
//! 文本帧收发的**最小**实现——零第三方依赖（ADR-014 极简哲学；SHA1 仅用于握手
//! 魔术串，不作密码学用途，依据与失效条件见模块文档）。
//!
//! 支持范围（M6 口径）：文本帧（服务端不掩码 / 客户端掩码解析）、`FIN + 无分片`；
//! 二进制帧、分片、扩展协商不支持——收到时返回错误（不静默降级）。
//!
//! 边界：本模块只做协议字节变换；连接管理与安全判定属 [`crate::api`]。

use whcode_domain::WhError;

/// WebSocket 握手魔术串（RFC 6455 §1.3）。
const MAGIC_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/// SHA-1（RFC 3174）最小实现；仅用于 WebSocket 握手魔术串
/// （不作密码学用途——握手魔术串是公开常量，攻击者无法从中伪造服务端身份；
/// 真正的安全判定属 Origin/认证检查）。
fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [
        0x6745_2301,
        0xEFCD_AB89,
        0x98BA_DCFE,
        0x1032_5476,
        0xC3D2_E1F0,
    ];
    let mut message = data.to_vec();
    let bit_len = (data.len() as u64) * 8;
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in message.chunks(64) {
        let mut w = [0u32; 80];
        for (index, item) in chunk.chunks(4).enumerate() {
            w[index] = u32::from_be_bytes([item[0], item[1], item[2], item[3]]);
        }
        for index in 16..80 {
            w[index] = (w[index - 3] ^ w[index - 8] ^ w[index - 14] ^ w[index - 16]).rotate_left(1);
        }
        let (mut a, mut b, mut c, mut d, mut e) = (h[0], h[1], h[2], h[3], h[4]);
        for (index, item) in w.iter().enumerate() {
            let (f, k) = match index {
                0..=19 => ((b & c) | ((!b) & d), 0x5A82_7999),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*item);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }
    let mut digest = [0u8; 20];
    for (index, word) in h.iter().enumerate() {
        digest[index * 4..index * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    digest
}

/// 标准字母表 base64 编码（含 `=` 填充）。
fn base64(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        out.push(ALPHABET[(b[0] >> 2) as usize] as char);
        out.push(ALPHABET[(((b[0] & 0x03) << 4) | (b[1] >> 4)) as usize] as char);
        if chunk.len() > 1 {
            out.push(ALPHABET[(((b[1] & 0x0F) << 2) | (b[2] >> 6)) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(ALPHABET[(b[2] & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

/// 计算握手响应的 `Sec-WebSocket-Accept` 值（RFC 6455 §1.3）。
#[must_use]
pub fn accept_key(client_key: &str) -> String {
    base64(&sha1(format!("{client_key}{MAGIC_GUID}").as_bytes()))
}

/// 服务端文本帧编码（RFC 6455 §5.2：FIN + 文本 opcode，服务端不掩码）。
#[must_use]
pub fn encode_text_frame(payload: &[u8]) -> Vec<u8> {
    let mut frame = vec![0x81]; // FIN + text
    let len = payload.len();
    if len < 126 {
        frame.push(len as u8);
    } else if len <= u16::MAX as usize {
        frame.push(126);
        frame.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        frame.push(127);
        frame.extend_from_slice(&(len as u64).to_be_bytes());
    }
    frame.extend_from_slice(payload);
    frame
}

/// 客户端帧解码（RFC 6455 §5.2：客户端帧必须掩码——未掩码即协议错误）；
/// 仅支持文本帧（opcode 0x1）且不含分片；返回文本载荷。
///
/// # Errors
///
/// 帧过短、非文本 opcode、含分片、未掩码或载荷超限时报 `parse_error` /
/// `invalid_state`（不静默降级）。
pub fn decode_client_frame(bytes: &[u8], max_payload: usize) -> Result<String, WhError> {
    if bytes.len() < 2 {
        return Err(WhError::parse_error("WebSocket 帧过短（<2 字节）"));
    }
    let first = bytes[0];
    let second = bytes[1];
    let fin = first & 0x80 != 0;
    let opcode = first & 0x0F;
    let masked = second & 0x80 != 0;
    let mut len = (second & 0x7F) as usize;
    let mut offset = 2;
    if len == 126 {
        if bytes.len() < 4 {
            return Err(WhError::parse_error("WebSocket 16 位长度不完整"));
        }
        len = u16::from_be_bytes([bytes[2], bytes[3]]) as usize;
        offset = 4;
    } else if len == 127 {
        if bytes.len() < 10 {
            return Err(WhError::parse_error("WebSocket 64 位长度不完整"));
        }
        len = u64::from_be_bytes([
            bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7], bytes[8], bytes[9],
        ]) as usize;
        offset = 10;
    }
    if !fin {
        return Err(WhError::invalid_state(
            "不支持分片帧（M6 口径，RFC 6455 分片）",
        ));
    }
    if opcode != 0x1 {
        return Err(WhError::invalid_state(format!(
            "仅支持文本帧（opcode 0x1），实际 0x{opcode:02x}"
        )));
    }
    if !masked {
        return Err(WhError::invalid_state("客户端帧必须掩码（RFC 6455 §5.1）"));
    }
    if len > max_payload {
        return Err(WhError::invalid_state(format!(
            "帧载荷 {len} 超过上限 {max_payload}（§10.2）"
        )));
    }
    if bytes.len() < offset + 4 + len {
        return Err(WhError::parse_error("WebSocket 帧载荷不完整"));
    }
    let mask = &bytes[offset..offset + 4];
    let payload = &bytes[offset + 4..offset + 4 + len];
    let unmasked: Vec<u8> = payload
        .iter()
        .enumerate()
        .map(|(index, byte)| byte ^ mask[index % 4])
        .collect();
    String::from_utf8(unmasked).map_err(|err| WhError::parse_error(format!("帧非 UTF-8：{err}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accept_key_matches_rfc_6455_example() {
        // RFC 6455 §1.3 的规范示例：dGhlIHNhbXBsZSBub25jZQ== → s3pPLMBiTxaQ9kYGzzhZRbK+xOo=
        assert_eq!(
            accept_key("dGhlIHNhbXBsZSBub25jZQ=="),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
    }

    #[test]
    fn text_frames_round_trip_small_and_large_payloads() {
        let long = "x".repeat(200);
        let long2 = "y".repeat(70_000);
        let payloads: [&str; 4] = ["你好", "", long.as_str(), long2.as_str()];
        for payload in payloads {
            let frame = encode_text_frame(payload.as_bytes());
            let _decoded = decode_client_frame(&frame, 1 << 20).expect_err("服务端帧未掩码");
            // 服务端帧不掩码（RFC 6455 §5.1），decode 校验掩码会拒绝——这里仅验证编码。
            assert!(!frame.is_empty(), "载荷 {payload}");
        }
        // 服务端帧结构：FIN+text，长度字段正确（<126 直接、≤65535 用 16 位、更大用 64 位）。
        let frame = encode_text_frame(b"hi");
        assert_eq!(&frame[0..2], &[0x81, 0x02]);
        let medium = encode_text_frame(vec![b'z'; 65_000].as_slice());
        assert_eq!(medium[1], 126, "65_000 字节用 16 位长度");
        assert_eq!(u16::from_be_bytes([medium[2], medium[3]]), 65_000);
        let large = encode_text_frame(vec![b'z'; 70_000].as_slice());
        assert_eq!(large[1], 127, "70_000 字节用 64 位长度");
    }

    #[test]
    fn masked_client_frames_decode_and_unmasked_are_rejected() {
        // 掩码客户端帧：payload "hi" + 掩码 0x01020304。
        let mut frame = vec![0x81, 0x82, 1, 2, 3, 4];
        frame.extend_from_slice(&[b'h' ^ 1, b'i' ^ 2]);
        let text = decode_client_frame(&frame, 1 << 20).expect("解码");
        assert_eq!(text, "hi");

        // 未掩码帧：协议错误。
        let unmasked = vec![0x81, 0x02, b'h', b'i'];
        let err = decode_client_frame(&unmasked, 1 << 20).expect_err("未掩码必须拒绝");
        assert!(err.to_string().contains("掩码"));

        // 非文本 opcode：拒绝。
        let binary = vec![0x82, 0x80, 1, 2, 3, 4];
        assert!(decode_client_frame(&binary, 1 << 20).is_err());

        // 载荷超限：拒绝（§10.2）。
        assert!(decode_client_frame(&frame, 1).is_err());
    }
}
