//! Wire encoding of `Semantics` (the `SetSemantics` payload after the id).
//!
//! Field order, as in spec §10.5: role u8, label Option<string>,
//! description Option<string>, live u8, flags u8, toggled u8, expanded u8,
//! value Option<(f32 now, f32 min, f32 max)>, value_text Option<string>,
//! position Option<(u32 index, u32 size)>, level u8, labelled_by
//! Option<u32>, described_by Option<u32>, controls Option<u32>.

use bytes::{Buf, BufMut, BytesMut};

use super::{decode_option_string, encode_option_string, validate_f32, Decode, Encode};
use crate::{
    semantics::{Expanded, Live, Role, SemanticValue, Semantics, SetPosition, Toggled},
    ProtocolError, RegionId, Result, SEMANTICS_FLAGS_ALL,
};

fn need(buf: &impl Buf, needed: usize) -> Result<()> {
    if buf.remaining() < needed {
        return Err(ProtocolError::UnexpectedEof {
            needed,
            have: buf.remaining(),
        });
    }
    Ok(())
}

fn get_u8(buf: &mut impl Buf) -> Result<u8> {
    need(buf, 1)?;
    Ok(buf.get_u8())
}

/// Read an option tag: `false` for absent, `true` for present.
fn get_option_tag(buf: &mut impl Buf) -> Result<bool> {
    match get_u8(buf)? {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(ProtocolError::InvalidData("invalid option tag".into())),
    }
}

fn invalid(field: &str, value: u8) -> ProtocolError {
    ProtocolError::InvalidData(format!("invalid semantics {field}: {value}"))
}

fn encode_option_region(id: Option<RegionId>, buf: &mut BytesMut) {
    match id {
        Some(id) => {
            buf.put_u8(1);
            id.encode(buf);
        }
        None => buf.put_u8(0),
    }
}

fn decode_option_region(buf: &mut impl Buf) -> Result<Option<RegionId>> {
    if get_option_tag(buf)? {
        Ok(Some(RegionId::decode(buf)?))
    } else {
        Ok(None)
    }
}

impl Encode for Semantics {
    fn encode(&self, buf: &mut BytesMut) {
        buf.put_u8(self.role as u8);
        encode_option_string(&self.label, buf);
        encode_option_string(&self.description, buf);
        buf.put_u8(self.live as u8);
        buf.put_u8(self.flags());
        buf.put_u8(self.toggled as u8);
        buf.put_u8(self.expanded as u8);
        match self.value {
            Some(value) => {
                buf.put_u8(1);
                buf.put_f32_le(value.now);
                buf.put_f32_le(value.min);
                buf.put_f32_le(value.max);
            }
            None => buf.put_u8(0),
        }
        encode_option_string(&self.value_text, buf);
        match self.position {
            Some(position) => {
                buf.put_u8(1);
                buf.put_u32_le(position.index);
                buf.put_u32_le(position.size);
            }
            None => buf.put_u8(0),
        }
        buf.put_u8(self.level);
        encode_option_region(self.labelled_by, buf);
        encode_option_region(self.described_by, buf);
        encode_option_region(self.controls, buf);
    }
}

impl Decode for Semantics {
    fn decode(buf: &mut impl Buf) -> Result<Self> {
        let role = get_u8(buf)?;
        let role = Role::from_u8(role).ok_or_else(|| invalid("role", role))?;
        let label = decode_option_string(buf)?;
        let description = decode_option_string(buf)?;
        let live = get_u8(buf)?;
        let live = Live::from_u8(live).ok_or_else(|| invalid("live", live))?;
        let flags = get_u8(buf)?;
        if flags & !SEMANTICS_FLAGS_ALL != 0 {
            return Err(invalid("flags", flags));
        }
        let toggled = get_u8(buf)?;
        let toggled = Toggled::from_u8(toggled).ok_or_else(|| invalid("toggled", toggled))?;
        let expanded = get_u8(buf)?;
        let expanded = Expanded::from_u8(expanded).ok_or_else(|| invalid("expanded", expanded))?;
        let value = if get_option_tag(buf)? {
            need(buf, 12)?;
            Some(SemanticValue {
                now: validate_f32(buf.get_f32_le())?,
                min: validate_f32(buf.get_f32_le())?,
                max: validate_f32(buf.get_f32_le())?,
            })
        } else {
            None
        };
        let value_text = decode_option_string(buf)?;
        let position = if get_option_tag(buf)? {
            need(buf, 8)?;
            Some(SetPosition {
                index: buf.get_u32_le(),
                size: buf.get_u32_le(),
            })
        } else {
            None
        };
        let level = get_u8(buf)?;
        let labelled_by = decode_option_region(buf)?;
        let described_by = decode_option_region(buf)?;
        let controls = decode_option_region(buf)?;

        let mut semantics = Semantics {
            role,
            label,
            description,
            live,
            toggled,
            expanded,
            value,
            value_text,
            position,
            level,
            labelled_by,
            described_by,
            controls,
            ..Semantics::default()
        };
        semantics.set_flags(flags);
        Ok(semantics)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AppMessage;

    fn full() -> Semantics {
        Semantics {
            role: Role::TreeItem,
            label: Some("src".to_string()),
            description: Some("source folder".to_string()),
            live: Live::Polite,
            busy: true,
            modal: true,
            toggled: Toggled::Mixed,
            expanded: Expanded::Collapsed,
            value: Some(SemanticValue {
                now: 0.5,
                min: 0.0,
                max: 1.0,
            }),
            value_text: Some("half".to_string()),
            position: Some(SetPosition { index: 2, size: 7 }),
            level: 3,
            labelled_by: Some(RegionId(4)),
            described_by: None,
            controls: Some(RegionId(9)),
            ..Semantics::default()
        }
    }

    /// Byte offsets of the one-byte fields in `minimal_bytes()`.
    const ROLE: usize = 4;
    const LIVE: usize = 7;
    const FLAGS: usize = 8;
    const TOGGLED: usize = 9;
    const EXPANDED: usize = 10;

    /// `SetSemantics` payload with every option absent: id, then fields.
    fn minimal_bytes() -> Vec<u8> {
        let mut buf = BytesMut::new();
        RegionId(1).encode(&mut buf);
        Semantics::default().encode(&mut buf);
        buf.to_vec()
    }

    #[test]
    fn default_semantics_has_golden_bytes() {
        assert_eq!(
            minimal_bytes(),
            [
                1, 0, 0, 0, // id
                0, // role Auto
                0, // label None
                0, // description None
                0, // live Off
                0, // flags
                0, // toggled Unset
                0, // expanded Unset
                0, // value None
                0, // value_text None
                0, // position None
                0, // level
                0, // labelled_by None
                0, // described_by None
                0, // controls None
            ]
        );
    }

    #[test]
    fn full_semantics_round_trips() {
        let semantics = full();
        let message = AppMessage::SetSemantics {
            id: RegionId(3),
            semantics: semantics.clone(),
        };
        let bytes = message.encode_to_bytes();
        let decoded = AppMessage::decode(&mut bytes.as_ref()).unwrap();
        assert_eq!(decoded, message);
    }

    fn decode_payload(bytes: &[u8]) -> Result<Semantics> {
        let mut cursor = &bytes[4..];
        Semantics::decode(&mut cursor)
    }

    #[test]
    fn unknown_enum_values_and_flag_bits_are_rejected() {
        assert!(decode_payload(&minimal_bytes()).is_ok());
        for (offset, bad) in [
            (ROLE, 21),
            (ROLE, 255),
            (LIVE, 3),
            (FLAGS, 64),
            (FLAGS, 128),
            (TOGGLED, 4),
            (EXPANDED, 3),
        ] {
            let mut bytes = minimal_bytes();
            bytes[offset] = bad;
            assert!(
                matches!(decode_payload(&bytes), Err(ProtocolError::InvalidData(_))),
                "byte {offset} = {bad} must be rejected"
            );
        }
    }

    #[test]
    fn non_finite_values_are_rejected() {
        for field in 0..3 {
            for bad in [f32::NAN, f32::INFINITY] {
                let mut semantics = full();
                let value = semantics.value.as_mut().unwrap();
                *[&mut value.now, &mut value.min, &mut value.max][field] = bad;
                let mut buf = BytesMut::new();
                semantics.encode(&mut buf);
                assert!(
                    matches!(
                        Semantics::decode(&mut buf.as_ref()),
                        Err(ProtocolError::InvalidFloat(_))
                    ),
                    "value field {field} = {bad} must be rejected"
                );
            }
        }
    }

    #[test]
    fn truncated_payloads_are_errors_not_panics() {
        let mut buf = BytesMut::new();
        full().encode(&mut buf);
        for len in 0..buf.len() {
            assert!(Semantics::decode(&mut &buf[..len]).is_err(), "len {len}");
        }
    }
}
