use crate::{
    engine::Engine,
    ecs::{InputComponent, GlobalComponent, HapticCmd, InFlight, systems::input_system::GILRS},
};
use gilrs::{Gilrs, GamepadId};
use gilrs::ff::{Effect, EffectBuilder, BaseEffect, BaseEffectType, Replay, Ticks};
use once_cell::sync::Lazy;
use std::{
    sync::Mutex,
    time::{Duration, Instant},
};
use anyhow::Result;

pub fn haptics_system(engine: &mut Engine) -> Result<()> {
    let mut gilrs = GILRS.lock().unwrap();

    let input_component = engine.get_global_component_mut::<InputComponent>()?;

    // Because some controllers (e.g. Xbox360) do not report when an effect has completed
    // Remove any in-flight effects that have completed
    {
        let now = std::time::Instant::now();
        input_component.in_flight_ff.retain(|in_flight| {
            if now >= in_flight.end_at {
                false
            } else {
                true
            }
        });
    }

    while let Some(cmd) = input_component.haptics.pop_front() {
        let index = cmd_player_index(&cmd)?;
        let Some(gamepad_id) = input_component.gamepad_ids.get(index).unwrap() else {
            continue; // No gamepad mapped for this player
        };

        // Is the haptic feedback supported?
        {
            let gamepad = gilrs.gamepad(*gamepad_id);
            if !gamepad.is_ff_supported() {
                continue;
            }
        }

        match cmd {
            HapticCmd::Rumble { player_id: _, weak, strong, duration_ms } => {
                let effect = create_rumble_effect(&mut gilrs, &[*gamepad_id], weak, strong, duration_ms)?;
                effect.play()?;
                let end_at = Instant::now() + Duration::from_millis(duration_ms as u64);
                input_component.in_flight_ff.push(InFlight { id: *gamepad_id, effect, end_at });
            },
            HapticCmd::PlayEffect { player_id: _, effect, duration_ms } => {
                let gamepad = gilrs.gamepad(*gamepad_id);
                effect.add_gamepad(&gamepad)?;
                effect.play()?;
                let end_at = Instant::now() + Duration::from_millis(duration_ms as u64);
                input_component.in_flight_ff.push(InFlight { id: *gamepad_id, effect, end_at });
            },
        }
    }
    Ok(())
}

#[inline]
fn cmd_player_index(cmd: &HapticCmd) -> Result<usize> {
    let pid = match cmd {
        HapticCmd::Rumble { player_id, .. } |
        HapticCmd::PlayEffect { player_id, .. } => *player_id,
    };
    Ok(pid as usize)
}

fn create_rumble_effect(gilrs: &mut Gilrs, recipients: &[GamepadId], weak: f32, strong: f32, duration_ms: u32) -> Result<Effect> {
    let dur = Ticks::from_ms(duration_ms);
    let effect = EffectBuilder::new()
        .add_effect(BaseEffect {
            kind: BaseEffectType::Weak { magnitude: (weak.clamp(0.0, 1.0) * 65_535.0) as u16 },
            scheduling: Replay { play_for: dur, ..Default::default() },
            ..Default::default()
        })
        .add_effect(BaseEffect {
            kind: BaseEffectType::Strong { magnitude: (strong.clamp(0.0, 1.0) * 65_535.0) as u16 },
            scheduling: Replay { play_for: dur, ..Default::default() },
            ..Default::default()
        })
        .gamepads(recipients)
        .finish(gilrs)?;
    Ok(effect)
}
