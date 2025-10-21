use anyhow::{anyhow, Result};
use log::{info, debug};
use mlua::{AnyUserData, Function, Lua, Table, UserData, UserDataMethods, Error as LuaError};
use std::cell::RefCell;

use crate::engine::Engine;
use crate::ecs::UpdatePhase;

// Thread-local VM so we avoid Send/Sync constraints.
thread_local! {
    static LUA_VM: RefCell<Option<LuaVm>> = const { RefCell::new(None) };
}

struct LuaVm {
    lua: Lua,
    started: bool,
    start_fn: Option<Function>,
    update_fn: Option<Function>,
}

impl LuaVm {
    fn new() -> mlua::Result<Self> {
        let lua = Lua::new();
        let g = lua.globals();
        let positions = lua.create_table()?;
        g.set("_positions", positions)?;
        Ok(Self { lua, started: false, start_fn: None, update_fn: None })
    }

    fn load_script(&mut self, src: &str) -> mlua::Result<()> {
        self.lua.load(src).set_name("script.lua").exec()?;
        let g = self.lua.globals();
        self.start_fn  = Some(g.get::<Function>("Start")?);
        self.update_fn = Some(g.get::<Function>("OnUpdate")?);
        Ok(())
    }
}

// Short-lived context passed to Lua.
struct LuaCtx { engine_ptr: *mut Engine }
impl LuaCtx { #[inline] unsafe fn engine(&self) -> &mut Engine { &mut *self.engine_ptr } }

impl UserData for LuaCtx {
    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method("log", |_, _this, msg: String| { info!("[lua] {msg}"); Ok(()) });

        // Seconds
        m.add_method("dt", |_, this, ()| {
            let e = unsafe { this.engine() };
            Ok((e.frame_delta_time / 1000.0) as f32)
        });

        // Spawn entity -> i64 index (MVP)
        m.add_method_mut("spawn", |_, this, ()| {
            let e = unsafe { this.engine() };
            let scene = e.get_active_scene_handle().map_err(LuaError::external)?;
            let ent   = e.create_entity(scene).map_err(LuaError::external)?;
            let idx   = unsafe { ent.get_data().index as i64 }; // use get_data() (no trait import needed)
            debug!("[Lua] spawned entity #{idx}");
            Ok(idx)
        });

        // MVP “position” mirror living in Lua globals (stub until you wire Transform)
        m.add_method_mut("get_x", |lua, _this, id: i64| {
            let positions: Table = lua.globals().get("_positions")?;
            let x: Option<f32> = positions.get(id)?;
            Ok(x.unwrap_or(0.0))
        });
        m.add_method_mut("set_x", |lua, _this, (id, x): (i64, f32)| {
            let positions: Table = lua.globals().get("_positions")?;
            positions.set(id, x)?;
            Ok(())
        });
    }
}

/// Call from `Engine::initialize`.
pub fn setup(engine: &mut Engine) -> Result<()> {
    LUA_VM.with(|cell| -> Result<()> {
        if cell.borrow().is_none() {
            let mut vm = LuaVm::new().map_err(|e| anyhow!("mlua init: {e}"))?;
            vm.load_script(DEMO_LUA).map_err(|e| anyhow!("mlua load: {e}"))?;
            *cell.borrow_mut() = Some(vm);
        }
        Ok(())
    })?;

    engine.system_manager.add_system("LuaScriptSystem", lua_system, UpdatePhase::Game)?;
    Ok(())
}

// Make this pub if your `ecs::mod.rs` re-exports it.
pub fn lua_system(engine: &mut Engine) -> Result<()> {
    LUA_VM.with(|cell| -> Result<()> {
        let mut vm_borrow = cell.borrow_mut();
        let vm = vm_borrow.as_mut().ok_or_else(|| anyhow!("Lua VM not initialized"))?;

        vm.lua.scope(|scope| -> mlua::Result<()> {
            let ctx_ud: AnyUserData = scope.create_userdata(LuaCtx { engine_ptr: engine as *mut Engine })?;
            let g = vm.lua.globals();
            g.set("ctx", ctx_ud.clone())?;

            if !vm.started {
                let start = vm.start_fn.as_ref().ok_or_else(|| LuaError::runtime("Start() not defined"))?;
                start.call::<()>(ctx_ud.clone())?;
                vm.started = true;
            }

            let upd = vm.update_fn.as_ref().ok_or_else(|| LuaError::runtime("OnUpdate() not defined"))?;
            upd.call::<()>(ctx_ud)?;
            Ok(())
        }).map_err(|e| anyhow!("mlua: {e}"))?;

        Ok(())
    })
}

// Spawns once, increments fake x every frame and logs it.
const DEMO_LUA: &str = r#"
local entity = nil

function Start(ctx)
  ctx:log("Start()")
  entity = ctx:spawn()
  ctx:set_x(entity, 0.0)
  ctx:log("Spawned entity: " .. tostring(entity))
end

function OnUpdate(ctx)
  local dt = ctx:dt()
  local x = ctx:get_x(entity)
  x = x + 2.0 * dt
  ctx:set_x(entity, x)
  ctx:log(string.format("OnUpdate: entity=%d x=%.3f dt=%.3f", entity, x, dt))
end
"#;

