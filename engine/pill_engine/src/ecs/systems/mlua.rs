use anyhow::{anyhow, Result};
use log::{info, debug, warn, error};
use mlua::{AnyUserData, Function, Lua, UserData, UserDataMethods, Error as LuaError};
use std::cell::RefCell;

use crate::engine::Engine;
use crate::ecs::{UpdatePhase, *};
use pill_core::{Vector3f, PillSlotMapKey};

use crate::graphics::*;
use crate::internal::{Mesh, Material};

// Thread-local VM so we avoid Send/Sync constraints.
thread_local! {
    static LUA_VM: RefCell<Option<LuaVm>> = RefCell::new(None);
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

// --------- Entity userdata (wraps your EntityHandle) -------------------------
#[derive(Clone, Copy)]
struct EntityUd { handle: crate::ecs::entity::EntityHandle }
impl UserData for EntityUd {} // marker only

// --------- Short-lived Ctx passed to Lua -------------------------------------
struct LuaCtx { engine_ptr: *mut Engine }
impl LuaCtx { #[inline] unsafe fn engine(&self) -> &mut Engine { &mut *self.engine_ptr } }

impl UserData for LuaCtx {
    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        // Logging + dt
        m.add_method("log", |_, _this, msg: String| { info!("[lua] {msg}"); Ok(()) });
        m.add_method("dt", |_, this, ()| {
            let e = unsafe { this.engine() };
            Ok((e.frame_delta_time / 1000.0) as f32)
        });

        // ---------------- SPAWN ----------------
        m.add_method_mut("spawn", |lua, this, ()| {
            let e = unsafe { this.engine() };
            let scene = e.get_active_scene_handle().map_err(LuaError::external)?;
            let handle = e.create_entity(scene).map_err(|err| {
                error!("[Lua] spawn failed: {err}");
                LuaError::external(err)
            })?;
            let idx = handle.data().index as i64;
            info!("[Lua] spawn: created entity handle index={idx}");
            let ud: AnyUserData = lua.create_userdata(EntityUd { handle })?;
            Ok(ud)
        });

        // --------------- TRANSFORM -------------
        m.add_method_mut("add_transform", |_, this, entity: AnyUserData| {
            let e = unsafe { this.engine() };
            let scene = e.get_active_scene_handle().map_err(LuaError::external)?;
            let h = *entity.borrow::<EntityUd>()?;
            match e.add_component_to_entity::<TransformComponent>(scene, h.handle, TransformComponent::new()) {
                Ok(_) => info!("[Lua] add_transform: OK for entity {}", h.handle.data().index),
                Err(err) => error!("[Lua] add_transform failed: {err}"),
            }
            Ok(())
        });

        m.add_method_mut("set_position", |_, this, (entity, x, y, z): (AnyUserData, f32, f32, f32)| {
            let e = unsafe { this.engine() };
            let h = *entity.borrow::<EntityUd>()?;
            let mut found = false;
            for (eh, tf) in e.iterate_one_component_mut::<TransformComponent>()
                .map_err(LuaError::external)?
            {
                if eh == h.handle {
                    tf.set_position(Vector3f::new(x, y, z));
                    info!("[Lua] set_position: entity {} -> ({:.2},{:.2},{:.2})",
                          h.handle.data().index, x, y, z);
                    found = true;
                    break;
                }
            }
            if !found {
                error!("[Lua] set_position: entity {} has no TransformComponent",
                       h.handle.data().index);
                return Err(LuaError::runtime("set_position: missing TransformComponent"));
            }
            Ok(())
        });

        m.add_method_mut("translate", |_, this, (entity, dx, dy, dz): (AnyUserData, f32, f32, f32)| {
            let e = unsafe { this.engine() };
            let h = *entity.borrow::<EntityUd>()?;
            let mut found = false;
            for (eh, tf) in e.iterate_one_component_mut::<TransformComponent>()
                .map_err(LuaError::external)?
            {
                if eh == h.handle {
                    let np = Vector3f::new(tf.position.x + dx, tf.position.y + dy, tf.position.z + dz);
                    tf.set_position(np);
                    debug!("[Lua] translate: entity {} by ({:.3},{:.3},{:.3}) -> now ({:.3},{:.3},{:.3})",
                        h.handle.data().index, dx, dy, dz, np.x, np.y, np.z);
                    found = true;
                    break;
                }
            }
            if !found {
                error!("[Lua] translate: entity {} has no TransformComponent",
                       h.handle.data().index);
                return Err(LuaError::runtime("translate: missing TransformComponent"));
            }
            Ok(())
        });

        // --------------- MESH/MATERIAL ---------
        m.add_method_mut("add_mesh_named", |_, this, (entity, mesh_name, material_name): (AnyUserData, String, String)| {
            let e = unsafe { this.engine() };
            let scene = e.get_active_scene_handle().map_err(LuaError::external)?;
            let h = *entity.borrow::<EntityUd>()?;

            let mesh_handle = match e.get_resource_handle::<Mesh>(&mesh_name) {
                Ok(h) => h,
                Err(err) => {
                    error!("[Lua] add_mesh_named: mesh '{}' not found: {err}", mesh_name);
                    return Err(LuaError::external(err));
                }
            };
            let material_handle = match e.get_resource_handle::<Material>(&material_name) {
                Ok(h) => h,
                Err(err) => {
                    error!("[Lua] add_mesh_named: material '{}' not found: {err}", material_name);
                    return Err(LuaError::external(err));
                }
            };

            let mrc = MeshRenderingComponent::builder()
                .mesh(&mesh_handle)
                .material(&material_handle)
                .build();

            match e.add_component_to_entity::<MeshRenderingComponent>(scene, h.handle, mrc) {
                Ok(_) => info!("[Lua] add_mesh_named: OK for entity {} (mesh='{}', material='{}')",
                               h.handle.data().index, mesh_name, material_name),
                Err(err) => {
                    error!("[Lua] add_mesh_named failed: {err}");
                    return Err(LuaError::external(err));
                }
            }
            Ok(())
        });

        // --------------- DEBUG DUMP ------------
        m.add_method("debug_dump", |_, this, ()| {
            let e = unsafe { this.engine() };
            let scene = match e.get_active_scene_handle() {
                Ok(h) => h,
                Err(err) => { error!("[Lua] debug_dump: no active scene: {err}"); return Ok(()); }
            };

            // Count transforms & renderers
            let mut t_count = 0usize;
            if let Ok(iter) = e.iterate_one_component::<TransformComponent>() {
                for _ in iter { t_count += 1; }
            }
            let mut mr_count = 0usize;
            if let Ok(iter) = e.iterate_one_component::<MeshRenderingComponent>() {
                for _ in iter { mr_count += 1; }
            }

            // Log first camera pose (if any)
            let mut logged_cam = false;
            if let Ok(mut it) = e.iterate_two_components::<TransformComponent, CameraComponent>() {
                if let Some((_eh, tf, _cam)) = it.next() {
                    info!("[Lua] debug: camera pose pos=({:.2},{:.2},{:.2}) rot=({:.1},{:.1},{:.1})",
                        tf.position.x, tf.position.y, tf.position.z,
                        tf.rotation.x, tf.rotation.y, tf.rotation.z);
                    logged_cam = true;
                }
            }
            if !logged_cam {
                warn!("[Lua] debug: no CameraComponent found");
            }

            info!("[Lua] debug: scene {:?}: transforms={}, mesh_renderers={}", scene, t_count, mr_count);
            info!("[Lua] debug: scene {:?}: transforms={}", scene, t_count);

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

pub fn lua_system(engine: &mut Engine) -> Result<()> {
    LUA_VM.with(|cell| -> Result<()> {
        let mut vm_borrow = cell.borrow_mut();
        let vm = vm_borrow.as_mut().ok_or_else(|| anyhow!("Lua VM not initialized"))?;

        vm.lua.scope(|scope| -> mlua::Result<()> {
            let ctx_ud: AnyUserData = scope.create_userdata(LuaCtx { engine_ptr: engine as *mut Engine })?;
            let g = vm.lua.globals();
            g.set("ctx", ctx_ud.clone())?;

            if !vm.started {
                info!("[Lua] Start()…");
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

// Demo: spawn, add Transform, (rendering) add pill+white, place at origin, move +X.
const DEMO_LUA: &str = r#"
local e = nil

function Start(ctx)
  ctx:log("Start()")
  ctx:debug_dump()

  e = ctx:spawn()
  ctx:add_transform(e)
  ctx:set_position(e, 0.0, 0.0, 0.0)

  -- If rendering build and "pill"/"white" exist (see Game::start), attach them:
  pcall(function() ctx:add_mesh_named(e, "pill", "white") end)

  ctx:debug_dump()
end

function OnUpdate(ctx)
  local dt = ctx:dt()
  -- slow move on X so it's easy to see
  ctx:translate(e, 1.0 * dt, 0.0, 0.0)
end
"#;

