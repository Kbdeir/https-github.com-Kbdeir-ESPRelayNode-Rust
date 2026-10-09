use crate::{
    command::{Action, Command, Source},
    protocol::command,
    runtime::RuntimeSnapshot,
};
use rmodbus::{
    server::{storage::ModbusStorage, ModbusFrame, Write},
    ErrorKind, ModbusProto,
};

pub const MAP_VERSION: u16 = 1;
pub fn frame_length(header: &[u8]) -> Result<usize, String> {
    if header.len() < 7 || header[2..4] != [0, 0] {
        return Err("invalid MBAP header".into());
    }
    let length = usize::from(u16::from_be_bytes([header[4], header[5]]));
    if !(2..=254).contains(&length) {
        return Err("invalid MBAP length".into());
    }
    Ok(6 + length)
}
pub fn respond<F>(
    bytes: &[u8],
    unit: u8,
    snapshot: &RuntimeSnapshot,
    mut apply: F,
) -> Result<Vec<u8>, String>
where
    F: FnMut(Command) -> Result<(), ErrorKind>,
{
    let state = snapshot.relay;
    let inputs = snapshot.inputs;
    let valid_mask = snapshot.input_valid_mask;
    let ttl = snapshot.ttl_seconds;
    let tta = snapshot.tta_seconds;
    if frame_length(bytes)? != bytes.len() || bytes[6] != unit {
        return Err("invalid frame size or unit".into());
    }
    let mut response = Vec::with_capacity(260);
    let mut frame = ModbusFrame::new(unit, bytes, ModbusProto::TcpUdp, &mut response);
    frame.parse().map_err(|_| "malformed Modbus request")?;
    if frame.processing_required {
        if frame.readonly {
            let mut context = ModbusStorage::<1, 6, 7, 7>::default();
            context.coils[0] = state.on;
            context.discretes = inputs;
            context.inputs = [
                (state.ttl_remaining_seconds >> 16) as u16,
                state.ttl_remaining_seconds as u16,
                (state.tta_remaining_seconds >> 16) as u16,
                state.tta_remaining_seconds as u16,
                (state.ttl_elapsed_seconds >> 16) as u16,
                state.ttl_elapsed_seconds as u16,
                valid_mask,
            ];
            context.holdings = [
                MAP_VERSION,
                (ttl >> 16) as u16,
                ttl as u16,
                (tta >> 16) as u16,
                tta as u16,
                u16::from(state.interlocked),
                valid_mask,
            ];
            frame
                .process_read(&context)
                .map_err(|_| "Modbus read failure")?;
        } else {
            let result = match frame.get_external_write() {
                Ok(Write::Bits(bits)) if bits.address == 0 && bits.count == 1 => apply(command(
                    if bits.data[0] & 1 == 1 {
                        Action::On
                    } else {
                        Action::Off
                    },
                    Source::Modbus,
                )),
                Ok(Write::Words(words)) if words.address == 1 && words.count == 2 => {
                    let value = u32::from_be_bytes(
                        words.data[..4]
                            .try_into()
                            .map_err(|_| "invalid register data")?,
                    );
                    if value > crate::config::MAX_TIMER_SECONDS {
                        Err(ErrorKind::IllegalDataValue)
                    } else {
                        apply(command(Action::SetTtl(value), Source::Modbus))
                    }
                }
                Ok(_) => Err(ErrorKind::IllegalDataAddress),
                Err(e) => Err(e),
            };
            frame
                .process_external_write(result)
                .map_err(|_| "Modbus write failure")?;
        }
    }
    if frame.response_required {
        frame
            .finalize_response()
            .map_err(|_| "Modbus response failure")?;
    }
    Ok(response)
}
