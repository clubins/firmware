//! Protobuf codec for the MQTT v2 contract (`api/mqtt/topics.md` +
//! `api/mqtt/v2/*.proto` in the source monorepo).
//!
//! The scaffold ships the wire primitives plus the `Header` and `Status`
//! messages, round-trip tested against the proto field numbers. The full
//! message set (per-type measurement messages) and fixture conformance land
//! with the MQTT transport task.

#![cfg_attr(not(test), no_std)]

use heapless::{String, Vec};

const WIRE_VARINT: u8 = 0;
const WIRE_FIXED64: u8 = 1;
const WIRE_BYTES: u8 = 2;

/// The encode buffer was too small for the message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BufFull;

/// The payload is not a valid message of the expected type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    UnexpectedEnd,
    MalformedVarint,
    /// A wire type no supported field uses (groups are not modelled).
    InvalidWireType,
    /// A boot_id that is not exactly 16 bytes.
    InvalidBootId,
    /// A string field on the wire that is not valid UTF-8.
    InvalidUtf8,
}

fn put_tag<const N: usize>(
    buf: &mut Vec<u8, N>,
    field: u32,
    wire: u8,
) -> Result<(), BufFull> {
    put_varint(buf, ((field << 3) | u32::from(wire)) as u64)
}

/// Base-128 varint encoding (protobuf wire format).
pub fn put_varint<const N: usize>(
    buf: &mut Vec<u8, N>,
    mut value: u64,
) -> Result<(), BufFull> {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            return buf.push(byte).map_err(|_| BufFull);
        }
        buf.push(byte | 0x80).map_err(|_| BufFull)?;
    }
}

fn put_fixed64<const N: usize>(buf: &mut Vec<u8, N>, value: u64) -> Result<(), BufFull> {
    for byte in value.to_le_bytes() {
        buf.push(byte).map_err(|_| BufFull)?;
    }
    Ok(())
}

fn put_bytes<const N: usize>(buf: &mut Vec<u8, N>, bytes: &[u8]) -> Result<(), BufFull> {
    put_varint(buf, bytes.len() as u64)?;
    for byte in bytes {
        buf.push(*byte).map_err(|_| BufFull)?;
    }
    Ok(())
}

/// Length-delimited buffer sizing for one encoded message.
pub const MESSAGE_BUFFER: usize = 128;

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn read_varint(&mut self) -> Result<u64, DecodeError> {
        let mut value: u64 = 0;
        let mut shift = 0;
        loop {
            let byte = *self.buf.get(self.pos).ok_or(DecodeError::UnexpectedEnd)?;
            self.pos += 1;
            value |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Ok(value);
            }
            shift += 7;
            if shift > 63 {
                return Err(DecodeError::MalformedVarint);
            }
        }
    }

    fn read_fixed64(&mut self) -> Result<u64, DecodeError> {
        let end = self.pos.checked_add(8).ok_or(DecodeError::UnexpectedEnd)?;
        let bytes = self
            .buf
            .get(self.pos..end)
            .ok_or(DecodeError::UnexpectedEnd)?;
        self.pos = end;
        Ok(u64::from_le_bytes(bytes.try_into().unwrap()))
    }

    fn read_bytes(&mut self) -> Result<&'a [u8], DecodeError> {
        let len = self.read_varint()? as usize;
        let end = self
            .pos
            .checked_add(len)
            .ok_or(DecodeError::UnexpectedEnd)?;
        let bytes = self
            .buf
            .get(self.pos..end)
            .ok_or(DecodeError::UnexpectedEnd)?;
        self.pos = end;
        Ok(bytes)
    }

    /// Skip a field we do not model (proto3 forward compatibility: new
    /// server-side fields must not break older firmware decoders).
    fn skip(&mut self, wire: u8) -> Result<(), DecodeError> {
        match wire {
            WIRE_VARINT => self.read_varint().map(|_| ()),
            WIRE_FIXED64 => self.read_fixed64().map(|_| ()),
            WIRE_BYTES => self.read_bytes().map(|_| ()),
            _ => Err(DecodeError::InvalidWireType),
        }
    }
}

/// `v2/header.proto` — shared by every message a sensor publishes.
///
/// `boot_id` and `seq` together are the deduplication key: a retransmission
/// carries the same pair; a gap in `seq` within one `boot_id` means messages
/// were lost (`topics.md` "Delivery").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    /// Unix epoch milliseconds UTC; **0 = clock not synchronised**.
    pub time_ms: u64,
    /// 16 random bytes from the hardware RNG, generated once per boot.
    pub boot_id: [u8; 16],
    /// Per-boot counter, per topic, starting at 0.
    pub seq: u32,
}

impl Header {
    pub fn encode<const N: usize>(&self, buf: &mut Vec<u8, N>) -> Result<(), BufFull> {
        put_tag(buf, 1, WIRE_FIXED64)?;
        put_fixed64(buf, self.time_ms)?;
        put_tag(buf, 2, WIRE_BYTES)?;
        put_bytes(buf, &self.boot_id)?;
        put_tag(buf, 3, WIRE_VARINT)?;
        put_varint(buf, u64::from(self.seq))?;
        Ok(())
    }

    pub fn decode(payload: &[u8]) -> Result<Self, DecodeError> {
        let mut reader = Reader::new(payload);
        let mut header = Self {
            time_ms: 0,
            boot_id: [0; 16],
            seq: 0,
        };
        while reader.pos < reader.buf.len() {
            let tag = reader.read_varint()?;
            let field = (tag >> 3) as u32;
            let wire = (tag & 0x7) as u8;
            match (field, wire) {
                (1, WIRE_FIXED64) => header.time_ms = reader.read_fixed64()?,
                (2, WIRE_BYTES) => {
                    let bytes = reader.read_bytes()?;
                    header.boot_id =
                        bytes.try_into().map_err(|_| DecodeError::InvalidBootId)?;
                }
                (3, WIRE_VARINT) => header.seq = reader.read_varint()? as u32,
                (_, wire) => reader.skip(wire)?,
            }
        }
        Ok(header)
    }
}

/// `status.proto` ConnectionState.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    /// Fall-back value, also for enum numbers a newer server defines.
    Unknown,
    /// Sensor is online.
    Online,
    /// Sensor is offline (successful disconnect).
    Offline,
    /// Sensor is offline (connection interrupted, power loss, etc).
    Died,
}

impl ConnectionState {
    fn to_proto(self) -> u32 {
        match self {
            ConnectionState::Unknown => 0,
            ConnectionState::Online => 1,
            ConnectionState::Offline => 2,
            ConnectionState::Died => 3,
        }
    }

    fn from_proto(value: u32) -> Self {
        match value {
            1 => ConnectionState::Online,
            2 => ConnectionState::Offline,
            3 => ConnectionState::Died,
            _ => ConnectionState::Unknown,
        }
    }
}

/// `v2/status.proto` — payload of `v2/sensors/<uuid>/status`, published once
/// per connection (retained).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub header: Header,
    /// When the sensor connected to the broker (0 while unsynchronised).
    pub connect_time_ms: u64,
    /// e.g. "0.1.0" or a git describe string.
    pub firmware_version: String<32>,
    pub connection_state: ConnectionState,
}

impl Status {
    pub fn encode<const N: usize>(&self, buf: &mut Vec<u8, N>) -> Result<(), BufFull> {
        let mut header_buf = Vec::<u8, MESSAGE_BUFFER>::new();
        self.header.encode(&mut header_buf)?;
        put_tag(buf, 1, WIRE_BYTES)?;
        put_bytes(buf, &header_buf)?;
        put_tag(buf, 2, WIRE_FIXED64)?;
        put_fixed64(buf, self.connect_time_ms)?;
        put_tag(buf, 3, WIRE_BYTES)?;
        put_bytes(buf, self.firmware_version.as_bytes())?;
        put_tag(buf, 4, WIRE_VARINT)?;
        put_varint(buf, u64::from(self.connection_state.to_proto()))?;
        Ok(())
    }

    pub fn decode(payload: &[u8]) -> Result<Self, DecodeError> {
        let mut reader = Reader::new(payload);
        let mut status = Self {
            header: Header {
                time_ms: 0,
                boot_id: [0; 16],
                seq: 0,
            },
            connect_time_ms: 0,
            firmware_version: String::new(),
            connection_state: ConnectionState::Unknown,
        };
        while reader.pos < reader.buf.len() {
            let tag = reader.read_varint()?;
            let field = (tag >> 3) as u32;
            let wire = (tag & 0x7) as u8;
            match (field, wire) {
                (1, WIRE_BYTES) => {
                    let bytes = reader.read_bytes()?;
                    status.header = Header::decode(bytes)?;
                }
                (2, WIRE_FIXED64) => status.connect_time_ms = reader.read_fixed64()?,
                (3, WIRE_BYTES) => {
                    let bytes = reader.read_bytes()?;
                    // heapless::String has no TryFrom<&[u8]>; validate UTF-8
                    // first so a corrupt string field is its own error.
                    let text = core::str::from_utf8(bytes)
                        .map_err(|_| DecodeError::InvalidUtf8)?;
                    status.firmware_version =
                        String::try_from(text).map_err(|_| DecodeError::UnexpectedEnd)?;
                }
                (4, WIRE_VARINT) => {
                    status.connection_state =
                        ConnectionState::from_proto(reader.read_varint()? as u32)
                }
                (_, wire) => reader.skip(wire)?,
            }
        }
        Ok(status)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_header() -> Header {
        let mut boot_id = [0u8; 16];
        for (index, byte) in boot_id.iter_mut().enumerate() {
            *byte = index as u8;
        }
        Header {
            time_ms: 1_724_000_000_123,
            boot_id,
            seq: 42,
        }
    }

    #[test]
    fn varint_round_trips_the_full_range() {
        for value in [0u64, 1, 127, 128, 300, u32::MAX as u64, u64::MAX] {
            let mut buf = Vec::<u8, 16>::new();
            put_varint(&mut buf, value).unwrap();
            let mut reader = Reader::new(&buf);
            assert_eq!(reader.read_varint().unwrap(), value);
            assert_eq!(reader.pos, buf.len());
        }
    }

    #[test]
    fn header_round_trips_against_the_proto_field_numbers() {
        let header = sample_header();
        let mut buf = Vec::<u8, MESSAGE_BUFFER>::new();
        header.encode(&mut buf).unwrap();
        assert_eq!(Header::decode(&buf).unwrap(), header);
    }

    #[test]
    fn header_decode_skips_unknown_fields() {
        // Encode a Header, then append an unknown field (field 4, varint 7).
        // proto3 decoders must skip it — a newer message stays readable.
        let header = sample_header();
        let mut buf = Vec::<u8, MESSAGE_BUFFER>::new();
        header.encode(&mut buf).unwrap();
        put_tag(&mut buf, 4, WIRE_VARINT).unwrap();
        put_varint(&mut buf, 7).unwrap();
        assert_eq!(Header::decode(&buf).unwrap(), header);
    }

    #[test]
    fn header_rejects_a_wrong_length_boot_id() {
        // Field 2 present but 4 bytes long, not 16.
        let mut buf = Vec::<u8, 16>::new();
        put_tag(&mut buf, 2, WIRE_BYTES).unwrap();
        put_bytes(&mut buf, &[1, 2, 3, 4]).unwrap();
        assert_eq!(Header::decode(&buf), Err(DecodeError::InvalidBootId));
    }

    #[test]
    fn status_round_trips_the_lifecycle_values() {
        for state in [
            ConnectionState::Unknown,
            ConnectionState::Online,
            ConnectionState::Offline,
            ConnectionState::Died,
        ] {
            let status = Status {
                header: sample_header(),
                connect_time_ms: 1_724_000_000_999,
                firmware_version: String::try_from("0.1.0").unwrap(),
                connection_state: state,
            };
            let mut buf = Vec::<u8, MESSAGE_BUFFER>::new();
            status.encode(&mut buf).unwrap();
            assert_eq!(Status::decode(&buf).unwrap(), status);
        }
    }

    #[test]
    fn unknown_connection_state_falls_back_to_unknown() {
        // Field 4 with an enum number a newer server defined (7): the proto
        // comment names Unknown the fall-back value.
        let status = Status {
            header: sample_header(),
            connect_time_ms: 0,
            firmware_version: String::try_from("0.1.0").unwrap(),
            connection_state: ConnectionState::Online,
        };
        let mut buf = Vec::<u8, MESSAGE_BUFFER>::new();
        status.encode(&mut buf).unwrap();
        // Overwrite the last varint (Online = 1) with 7.
        let last = buf.len() - 1;
        assert_eq!(buf[last], 1);
        buf[last] = 7;
        assert_eq!(
            Status::decode(&buf).unwrap().connection_state,
            ConnectionState::Unknown
        );
    }
}
