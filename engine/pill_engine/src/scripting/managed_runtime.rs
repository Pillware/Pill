use anyhow::Result;
use netcorehost::hostfxr::{
    AssemblyDelegateLoader, Hostfxr, HostfxrContext, InitializedForRuntimeConfig,
    ManagedFunctionWithDefaultSignature,
};
use netcorehost::pdcstring::{PdCStr, PdCString};
use netcorehost::{nethost, pdcstr};
use std::net::Shutdown;
use std::path::Path;

#[repr(C)]
struct LoadScriptsArgs {
    scripts_assembly_path_ptr: *const u8,
    scripts_assembly_path_len: i32,
}

#[repr(C)]
struct EntityArgs {
    entity: u64,
}

#[repr(C)]
struct UpdateScriptArgs {
    entity: u64,
    dt: f32,
}

#[repr(C)]
struct CreateScriptArgs {
    entity: u64,
    script_type_ptr: *const u8,
    script_type_len: i32,
}

type ManagedFn = ManagedFunctionWithDefaultSignature;

pub struct ManagedRuntime {
    // TODO: verify that we can drop these later
    hostfxr: Hostfxr,
    context: HostfxrContext<InitializedForRuntimeConfig>,
    fn_loader: AssemblyDelegateLoader,

    load_scripts: ManagedFn,
    shutdown: ManagedFn,
    create_script: ManagedFn,
    start_script: ManagedFn,
    update_script: ManagedFn,
    destroy_script: ManagedFn,
}

fn load_script_host_fn(
    fn_loader: &AssemblyDelegateLoader,
    method_name: &PdCStr,
) -> Result<ManagedFn> {
    Ok(fn_loader.get_function_with_default_signature(
        pdcstr!("Pill.ManagedHost.ScriptHost, Pill.ManagedHost"),
        method_name,
    )?)
}

impl ManagedRuntime {
    pub fn init(runtime_config: &Path, managed_assembly: &Path) -> Result<Self> {
        let runtime_config_pdc = PdCString::from_os_str(runtime_config.as_os_str())
            .expect("failed to convert runtime_config path");

        let managed_assembly_pdc = PdCString::from_os_str(managed_assembly.as_os_str())
            .expect("failed to convert managed_assembly path");

        let hostfxr = nethost::load_hostfxr().unwrap();

        let context = hostfxr
            .initialize_for_runtime_config(&runtime_config_pdc)
            .unwrap();

        let fn_loader = context
            .get_delegate_loader_for_assembly(managed_assembly_pdc)
            .unwrap();

        let load_scripts = load_script_host_fn(&fn_loader, pdcstr!("LoadScripts"))?;
        let shutdown = load_script_host_fn(&fn_loader, pdcstr!("Shutdown"))?;
        let create_script = load_script_host_fn(&fn_loader, pdcstr!("CreateScript"))?;
        let start_script = load_script_host_fn(&fn_loader, pdcstr!("StartScript"))?;
        let update_script = load_script_host_fn(&fn_loader, pdcstr!("UpdateScript"))?;
        let destroy_script = load_script_host_fn(&fn_loader, pdcstr!("DestroyScript"))?;

        Ok(Self {
            hostfxr,
            context,
            fn_loader,
            load_scripts,
            shutdown,
            create_script,
            start_script,
            update_script,
            destroy_script,
        })
    }

    pub fn load_scripts(&self, scripts_assembly: &Path) -> Result<()> {
        let scripts_assembly_s = scripts_assembly.to_str().unwrap();
        let args = LoadScriptsArgs {
            scripts_assembly_path_ptr: scripts_assembly_s.as_ptr(),
            scripts_assembly_path_len: scripts_assembly_s.len() as i32,
        };
        let result = unsafe {
            (self.load_scripts)(
                (&args as *const LoadScriptsArgs).cast_mut().cast(),
                std::mem::size_of::<LoadScriptsArgs>() as i32,
            )
        };

        if result != 0 {
            anyhow::bail!("LoadScripts failed with code {}", result);
        }
        Ok(())
    }

    pub fn shutdown(&mut self) -> Result<()> {
        let result = unsafe { (self.shutdown)(std::ptr::null_mut(), 0) };

        if result != 0 {
            anyhow::bail!("Shudown failed with code {}", result);
        }
        Ok(())
    }

    // TODO: change args
    pub fn create_script(&self, entity: u64, script_type: &str) -> Result<()> {
        let args = CreateScriptArgs {
            entity,
            script_type_ptr: script_type.as_ptr(),
            script_type_len: script_type.len() as i32,
        };

        let result = unsafe {
            (self.create_script)(
                (&args as *const CreateScriptArgs).cast_mut().cast(),
                std::mem::size_of::<CreateScriptArgs>() as i32,
            )
        };

        if result != 0 {
            anyhow::bail!("CreateScript failed with code {}", result);
        }

        Ok(())
    }

    // TODO: we might want to have more scripts attached to one entity
    pub fn start_script(&self, entity: u64) -> Result<()> {
        let args = EntityArgs { entity };

        let result = unsafe {
            // TODO: add the helper for calling?
            (self.start_script)(
                (&args as *const EntityArgs).cast_mut().cast(),
                std::mem::size_of::<EntityArgs>() as i32,
            )
        };

        if result != 0 {
            anyhow::bail!("StartScript failed with code {}", result);
        }

        Ok(())
    }

    pub fn update_script(&self, entity: u64, dt: f32) -> Result<()> {
        let args = UpdateScriptArgs { entity, dt };

        let result = unsafe {
            (self.update_script)(
                (&args as *const UpdateScriptArgs).cast_mut().cast(),
                std::mem::size_of::<UpdateScriptArgs>() as i32,
            )
        };

        if result != 0 {
            anyhow::bail!("UpdateScript failed with code {}", result);
        }

        Ok(())
    }

    pub fn destroy_script(&self, entity: u64) -> Result<()> {
        let args = EntityArgs { entity };

        let result = unsafe {
            (self.destroy_script)(
                (&args as *const EntityArgs).cast_mut().cast(),
                std::mem::size_of::<EntityArgs>() as i32,
            )
        };

        if result != 0 {
            anyhow::bail!("DestroyScript failed with code {}", result);
        }

        Ok(())
    }
}
