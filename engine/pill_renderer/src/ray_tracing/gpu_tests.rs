//! GPU validation tests for hardware ray tracing.
//!
//! These tests require a Vulkan adapter with `EXPERIMENTAL_RAY_QUERY`
//! support (e.g. NVIDIA RTX 3080 Ti). They skip gracefully when no
//! compatible hardware is available.
//!
//! Run with:
//!   cargo test -p pill_renderer --features hardware_ray_tracing -- gpu
//!   -- --test-threads=1 --nocapture

use super::blas::create_blas_size_descriptor;
use wgpu::util::DeviceExt;

// ── Helpers ────────────────────────────────────────────────────────────

fn create_rt_device() -> Option<(wgpu::Adapter, wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::VULKAN,
        flags: wgpu::InstanceFlags::from_build_config().with_env(),
        backend_options: wgpu::BackendOptions::default(),
        memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
        display: None,
    });

    let adapters = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::VULKAN));
    let adapter = adapters.into_iter().find(|a| {
        a.features().contains(wgpu::Features::EXPERIMENTAL_RAY_QUERY)
    })?;

    let info = adapter.get_info();
    eprintln!("RT test: adapter '{}' ({:?})", info.name, info.backend);

    let features = wgpu::Features::EXPERIMENTAL_RAY_QUERY & adapter.features();
    let limits = wgpu::Limits::default()
        .using_minimum_supported_acceleration_structure_values();

    let device_descriptor = wgpu::DeviceDescriptor {
        label: Some("rt_gpu_test"),
        required_features: features,
        required_limits: limits,
        experimental_features: unsafe { wgpu::ExperimentalFeatures::enabled() },
        memory_hints: wgpu::MemoryHints::default(),
        trace: wgpu::Trace::default(),
    };

    let (device, queue) =
        pollster::block_on(adapter.request_device(&device_descriptor)).ok()?;

    Some((adapter, device, queue))
}

fn read_u32_from_staging(staging: &wgpu::Buffer, device: &wgpu::Device) -> u32 {
    let slice = staging.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        tx.send(r).unwrap();
    });
    let _ = device.poll(wgpu::PollType::Wait {
        submission_index: None,
        timeout: None,
    });
    rx.recv().unwrap().unwrap();
    let view = slice.get_mapped_range().unwrap();
    let val = u32::from_le_bytes([view[0], view[1], view[2], view[3]]);
    drop(view);
    staging.unmap();
    val
}

fn check_validation_errors(rx: &std::sync::mpsc::Receiver<String>, label: &str) {
    while let Ok(err) = rx.try_recv() {
        eprintln!("VALIDATION [{}]: {}", label, err);
    }
}

// ── Tests ──────────────────────────────────────────────────────────────

#[test]
fn gpu_canary_compilation() {
    let Some((_adapter, device, _queue)) = create_rt_device() else {
        eprintln!("SKIP gpu_canary_compilation: no RT adapter");
        return;
    };
    let result = super::pipeline::compile_ray_query_canary(&device);
    assert!(result.is_ok(), "canary failed: {:?}", result.err());
    eprintln!("PASS: canary compilation");
}

#[test]
fn gpu_canary_missing_directive_fails() {
    let Some((_adapter, device, _queue)) = create_rt_device() else {
        eprintln!("SKIP gpu_canary_missing_directive: no RT adapter");
        return;
    };

    let (error_tx, error_rx) = std::sync::mpsc::channel();
    use std::sync::Arc;
    device.on_uncaptured_error(Arc::new(move |err| {
        let _ = error_tx.send(err.to_string());
    }));

    const NO_DIRECTIVE: &str = r#"
@group(0) @binding(0)
var<storage, read_write> output: u32;
@compute @workgroup_size(1)
fn main() {
    var rq: ray_query;
    _ = &rq;
    output = 0u;
}
"#;

    let _module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("no_directive"),
        source: wgpu::ShaderSource::Wgsl(NO_DIRECTIVE.into()),
    });
    let _ = device.poll(wgpu::PollType::Poll);

    if let Ok(err_msg) = error_rx.try_recv() {
        eprintln!("PASS: missing directive rejected at module creation: {}", err_msg);
        return;
    }

    // Try pipeline creation.
    let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("nd_bgl"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: false },
                has_dynamic_offset: false,
                min_binding_size: std::num::NonZeroU64::new(4),
            },
            count: None,
        }],
    });
    let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("nd_pl"),
        bind_group_layouts: &[Some(&bgl)],
        immediate_size: 0,
    });
    let _pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("nd_pipeline"),
        layout: Some(&pl),
        module: &_module,
        entry_point: Some("main"),
        compilation_options: Default::default(),
        cache: None,
    });
    let _ = device.poll(wgpu::PollType::Poll);

    if let Ok(err_msg) = error_rx.try_recv() {
        eprintln!("PASS: missing directive rejected at pipeline creation: {}", err_msg);
        return;
    }

    // Final flush.
    let _ = device.poll(wgpu::PollType::Wait {
        submission_index: None,
        timeout: Some(std::time::Duration::from_millis(500)),
    });
    if let Ok(err_msg) = error_rx.try_recv() {
        eprintln!("PASS: missing directive rejected after wait: {}", err_msg);
        return;
    }

    panic!("Expected Naga to reject ray-query shader without `enable wgpu_ray_query;`");
}

#[test]
fn gpu_hit_and_miss() {
    let Some((_adapter, device, queue)) = create_rt_device() else {
        eprintln!("SKIP gpu_hit_and_miss: no RT adapter");
        return;
    };

    let (error_tx, error_rx) = std::sync::mpsc::channel();
    use std::sync::Arc;
    device.on_uncaptured_error(Arc::new(move |err| {
        let _ = error_tx.send(err.to_string());
    }));

    // --- Create triangle geometry ---
    let vertices: Vec<[f32; 3]> = vec![
        [-10.0, -10.0, 0.0],
        [10.0, -10.0, 0.0],
        [0.0, 10.0, 0.0],
    ];
    let indices: Vec<u32> = vec![0, 1, 2]; // front face +Z, ray from -Z hits front

    let vb = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("test_vb"),
        contents: bytemuck::cast_slice(&vertices),
        usage: wgpu::BufferUsages::BLAS_INPUT | wgpu::BufferUsages::COPY_DST,
    });
    let ib = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("test_ib"),
        contents: bytemuck::cast_slice(&indices),
        usage: wgpu::BufferUsages::BLAS_INPUT | wgpu::BufferUsages::COPY_DST,
    });

    let size_desc = create_blas_size_descriptor(
        vertices.len() as u32,
        indices.len() as u32,
        wgpu::IndexFormat::Uint32,
    );

    let blas = device.create_blas(
        &wgpu::CreateBlasDescriptor {
            label: Some("test_blas"),
            flags: wgpu::AccelerationStructureFlags::PREFER_FAST_TRACE,
            update_mode: wgpu::AccelerationStructureUpdateMode::Build,
        },
        wgpu::BlasGeometrySizeDescriptors::Triangles {
            descriptors: vec![size_desc.clone()],
        },
    );

    // --- Create TLAS (empty initially) ---
    let mut tlas = device.create_tlas(&wgpu::CreateTlasDescriptor {
        label: Some("test_tlas"),
        max_instances: 1,
        flags: wgpu::AccelerationStructureFlags::PREFER_FAST_BUILD,
        update_mode: wgpu::AccelerationStructureUpdateMode::Build,
    });

    // Set TLAS instance FIRST, then build both BLAS and TLAS in ONE encoder.
    let ident: [f32; 12] = [
        1.0, 0.0, 0.0, 0.0,
        0.0, 1.0, 0.0, 0.0,
        0.0, 0.0, 1.0, 0.0,
    ];
    *tlas.get_mut_single(0).unwrap() = Some(wgpu::TlasInstance::new(&blas, ident, 0, 0xff));
    eprintln!("TLAS instances set: {}", tlas.get().iter().filter(|i| i.is_some()).count());

    // --- Build BLAS and TLAS in ONE encoder + ONE submit ---
    let mut build_enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("combined_build"),
    });
    build_enc.build_acceleration_structures(
        &[wgpu::BlasBuildEntry {
            blas: &blas,
            geometry: wgpu::BlasGeometries::TriangleGeometries(vec![
                wgpu::BlasTriangleGeometry {
                    size: &size_desc,
                    vertex_buffer: &vb,
                    first_vertex: 0,
                    vertex_stride: std::mem::size_of::<[f32; 3]>() as u64,
                    index_buffer: Some(&ib),
                    first_index: Some(0),
                    transform_buffer: None,
                    transform_buffer_offset: None,
                },
            ]),
        }],
        std::slice::from_ref(&tlas),
    );
    queue.submit(std::iter::once(build_enc.finish()));
    let _ = device.poll(wgpu::PollType::Wait {
        submission_index: None,
        timeout: None,
    });
    check_validation_errors(&error_rx, "combined_build");
    eprintln!("Combined BLAS+TLAS build completed");

    // --- Output + staging ---
    let output = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("out"),
        size: 4,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("staging"),
        size: 4,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    // --- Bind group / pipeline ---
    let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("bgl"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: false },
                    has_dynamic_offset: false,
                    min_binding_size: std::num::NonZeroU64::new(4),
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::AccelerationStructure { vertex_return: false },
                count: None,
            },
        ],
    });

    let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("bg"),
        layout: &bgl,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: output.as_entire_binding() },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::AccelerationStructure(&tlas),
            },
        ],
    });

    let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("pl"),
        bind_group_layouts: &[Some(&bgl)],
        immediate_size: 0,
    });

    // --- Hit-test compute shader ---
    const HIT_SHADER: &str = r#"
enable wgpu_ray_query;

@group(0) @binding(0)
var<storage, read_write> output: u32;

@group(0) @binding(1)
var tlas: acceleration_structure;

@compute @workgroup_size(1)
fn main() {
    var rq: ray_query;
    var ray_desc: RayDesc;
    ray_desc.origin = vec3<f32>(0.0, 0.0, -5.0);
    ray_desc.dir = vec3<f32>(0.0, 0.0, 1.0);
    ray_desc.tmin = 0.0f;
    ray_desc.tmax = 100.0f;
    ray_desc.flags = 0u;   // no flags
    ray_desc.cull_mask = 0xffu;
    rayQueryInitialize(&rq, tlas, ray_desc);

    var hit: u32 = 0u;
    loop {
        if (rayQueryProceed(&rq)) {
            rayQueryConfirmIntersection(&rq);
            hit = 1u;
        } else {
            break;
        }
    }

    output = hit;
}
"#;

    let sm = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("sm"),
        source: wgpu::ShaderSource::Wgsl(HIT_SHADER.into()),
    });

    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("pipeline"),
        layout: Some(&pl),
        module: &sm,
        entry_point: Some("main"),
        compilation_options: Default::default(),
        cache: None,
    });

    // --- Dispatch hit test ---
    let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("hit_dispatch"),
    });
    enc.clear_buffer(&output, 0, Some(4));
    {
        let mut cp = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("hit_cp"),
            timestamp_writes: None,
        });
        cp.set_pipeline(&pipeline);
        cp.set_bind_group(0, &bg, &[]);
        cp.dispatch_workgroups(1, 1, 1);
    }
    enc.copy_buffer_to_buffer(&output, 0, &staging, 0, 4);
    queue.submit(std::iter::once(enc.finish()));
    let _ = device.poll(wgpu::PollType::Wait {
        submission_index: None,
        timeout: None,
    });
    check_validation_errors(&error_rx, "hit_dispatch");

    let hit_result = read_u32_from_staging(&staging, &device);
    eprintln!("Hit test result: {} (expected >=1 for hit, 0=miss)", hit_result);
    assert!(hit_result >= 1, "Expected triangle hit (>=1 candidates), got {}", hit_result);
    eprintln!("PASS: hit");

    // --- MISS test: move instance +10 in X ---
    let mut tlas2 = device.create_tlas(&wgpu::CreateTlasDescriptor {
        label: Some("test_tlas2"),
        max_instances: 1,
        flags: wgpu::AccelerationStructureFlags::PREFER_FAST_BUILD,
        update_mode: wgpu::AccelerationStructureUpdateMode::Build,
    });
    *tlas2.get_mut_single(0).unwrap() = Some(wgpu::TlasInstance::new(
        &blas,
        [1.0, 0.0, 0.0, 10.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0],
        0,
        0xff,
    ));

    let bg2 = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("bg2"),
        layout: &bgl,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: output.as_entire_binding() },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::AccelerationStructure(&tlas2),
            },
        ],
    });

    let mut enc2 = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("miss_dispatch"),
    });
    enc2.build_acceleration_structures(&[], std::slice::from_ref(&tlas2));
    enc2.clear_buffer(&output, 0, Some(4));
    {
        let mut cp = enc2.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("miss_cp"),
            timestamp_writes: None,
        });
        cp.set_pipeline(&pipeline);
        cp.set_bind_group(0, &bg2, &[]);
        cp.dispatch_workgroups(1, 1, 1);
    }
    enc2.copy_buffer_to_buffer(&output, 0, &staging, 0, 4);
    queue.submit(std::iter::once(enc2.finish()));
    let _ = device.poll(wgpu::PollType::Wait {
        submission_index: None,
        timeout: None,
    });
    check_validation_errors(&error_rx, "miss_dispatch");

    let miss_result = read_u32_from_staging(&staging, &device);
    eprintln!("Miss test result: {} (expected 0)", miss_result);
    assert_eq!(miss_result, 0, "Expected miss, got {}", miss_result);
    eprintln!("PASS: miss");
}
