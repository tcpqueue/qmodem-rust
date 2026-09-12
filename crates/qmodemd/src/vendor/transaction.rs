// SPDX-License-Identifier: GPL-3.0-only
// AT behavior ported from FUjr/QModem vendor/quectel.sh and vendor/huawei.sh.
// Copyright (C) 2023 Siriling <siriling@qq.com>
// Copyright (C) 2025 Fujr <fjrcn@outlook.com>
// Copyright (C) 2025 coolsnowwolf <coolsnowwolf@gmail.com>
// Rust adaptation Copyright (C) 2026 tcpqueue
use super::*;
use crate::at::{Program, Sequence};

pub fn local(device: &Modem, op: &Operation, runtime: &Runtime) -> Result<Option<Value>> {
    if family(device)? != Family::TdtechMt5700 {
        return Ok(None);
    }
    let data = match op {
        Operation::GetUsageStats => {
            json!({"available":false,"total_rx_bytes":0,"total_tx_bytes":0})
        }
        Operation::GetSimSlot => {
            json!({"sim_slot":runtime.slot(&device.id)?,"source":"software","hardware_verified":false})
        }
        Operation::GetSimCapabilities => {
            json!({"supported":true,"slots":[0,1],"source":"software","hardware_verified":false})
        }
        _ => return Ok(None),
    };
    Ok(Some(json!({"success":true,"data":data})))
}
pub fn plan(device: &Modem, op: &Operation, runtime: &Runtime) -> Result<Box<dyn Program>> {
    let family = family(device)?;
    let command = |c: &str| Step::command(c, Duration::from_secs(10));
    let (steps, continue_on_error) = match op {
        Operation::GetNeighborcell => (cells::query(device)?, true),
        Operation::SetCellLock { lock } => (vec![cells::lock(device, lock)?], false),
        Operation::UnlockCell => (cells::unlock(device)?, true),
        Operation::GetUsageStats | Operation::WriteUsageStats | Operation::ClearUsageStats => {
            (vec![usage::command(device, op)?], false)
        }
        Operation::GetBandLock => (bands::query(device)?, true),
        Operation::SetBandLock { band_class, bands } => {
            (vec![bands::setter(device, *band_class, bands)?], false)
        }
        Operation::GetSimCapabilities => {
            ensure!(
                family == Family::Quectel,
                "MT5700 capabilities are software-defined"
            );
            (vec![command("AT+QUIMSLOT=?")?], false)
        }
        Operation::SetSimSlot { slot } => {
            if family == Family::Quectel {
                ensure!([1, 2].contains(slot), "Quectel SIM slot must be 1 or 2");
                return Ok(Box::new(sim::Switch {
                    slot: *slot,
                    sent: false,
                    slept_after: 0,
                }));
            }
            ensure!(*slot <= 1, "MT5700 SIM slot must be 0 or 1");
            return Ok(Box::new(sim::SoftwareSwitch {
                runtime: runtime.clone(),
                id: device.id.clone(),
                slot: *slot,
                command: Some(command(if *slot == 0 {
                    "AT^SCICHG=0,1"
                } else {
                    "AT^SCICHG=1,0"
                })?),
            }));
        }
        Operation::SetImei { imei } => {
            ensure!(
                imei.len() == 15 && imei.bytes().all(|b| b.is_ascii_digit()),
                "IMEI must contain exactly 15 ASCII digits"
            );
            if family == Family::Quectel {
                (
                    vec![
                        command(&format!("AT+EGMR=1,7,\"{imei}\""))?,
                        command("AT+CGSN")?,
                    ],
                    true,
                )
            } else {
                (vec![command(&format!("at^phynum=IMEI,{imei}"))?], false)
            }
        }
        _ => (vec![prepare(device, op)?], false),
    };
    Ok(Box::new(Sequence::new(steps, continue_on_error)))
}
pub fn finish(device: &Modem, op: &Operation, replies: &[Reply]) -> Result<Value> {
    ensure!(!replies.is_empty(), "empty vendor transaction");
    match op {
        Operation::GetNeighborcell => cells::interpret(device, replies),
        Operation::GetUsageStats => Ok(
            json!({"success":true,"data":usage::interpret(device,&replies[0]),"replies":replies}),
        ),
        Operation::SetCellLock { .. }
        | Operation::UnlockCell
        | Operation::WriteUsageStats
        | Operation::ClearUsageStats => Ok(
            json!({"success":replies.iter().all(|r|r.modem_success),"data":{},"replies":replies}),
        ),
        Operation::GetBandLock => bands::interpret(device, replies),
        Operation::GetSimCapabilities => Ok(
            json!({"success":replies[0].modem_success,"data":sim::capabilities(&replies[0].response),"replies":replies}),
        ),
        Operation::SetSimSlot { slot } => {
            if family(device)? == Family::TdtechMt5700 {
                return Ok(
                    json!({"success":replies[0].modem_success,"data":{"sim_slot":slot,"source":"software","hardware_verified":false},"replies":replies}),
                );
            }
            let accepted = replies[0].response.lines().any(|l| l.starts_with("OK"));
            let current = if replies.len() > 1 {
                sim::parse_slot(&replies.last().unwrap().response)
            } else {
                None
            };
            Ok(
                json!({"success":accepted && current==Some(*slot),"error_code":if accepted{"sim_switch_unconfirmed"}else{"modem_rejected"},
                "data":{"requested_slot":slot,"sim_slot":current,"source":"modem","hardware_verified":current.is_some(),"attempts":replies.len()-1},"replies":replies}),
            )
        }
        Operation::SetImei { imei } => {
            let data = if family(device)? == Family::Quectel {
                ensure!(replies.len() == 2, "missing IMEI readback");
                let readback = interpret(device, &Operation::GetImei, &replies[1]);
                let observed = readback
                    .ok()
                    .and_then(|v| v["data"]["imei"].as_str().map(str::to_owned));
                json!({"imei":observed,"matches_requested":observed.as_deref()==Some(imei),"hardware_verified":observed.is_some()})
            } else {
                json!({"hardware_verified":false})
            };
            Ok(
                json!({"success":replies.iter().all(|r|r.modem_success) && (family(device)?!=Family::Quectel || data["matches_requested"]==true),"error_code":if replies.iter().any(|r|!r.modem_success){"modem_rejected"}else if data["imei"].is_null(){"invalid_modem_response"}else{"imei_unconfirmed"},"data":data,"replies":replies}),
            )
        }
        _ => {
            let mut result = interpret(device, op, &replies[0])?;
            result["replies"] = json!(replies);
            Ok(result)
        }
    }
}
