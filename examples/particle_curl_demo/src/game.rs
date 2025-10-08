use pill_engine::game::*;
use noise::{NoiseFn, OpenSimplex};
use rand::Rng;
use std::{collections::HashMap, sync::Arc};
use rayon::prelude::*;

const PARTICLE_COUNT: usize = 8000;

// / Helper functions
#[inline]
fn idx(nx: usize, ny: usize, x: usize, y: usize, z: usize) -> usize {
    z * ny * nx + y * nx + x
}

#[inline]
fn wrap(i: i32, n: usize) -> usize {
    let n_i = n as i32;
    ((i % n_i) + n_i) as usize % n
}

// Define custom component
pub struct PillComponent { }

impl Component for PillComponent { }

impl PillTypeMapKey for PillComponent {
    type Storage = ComponentStorage<Self>;
}

// ---- Particle Curl Implementation ----
#[derive(Clone, Copy)]
pub struct Particle;

impl Component for Particle { }
impl PillTypeMapKey for Particle {
    type Storage = ComponentStorage<Self>;
}

#[derive(Clone, Copy)]
pub struct Velocity(pub Vector3f);

impl Default for Velocity {
    fn default() -> Self {
        Self(Vector3f::new(0.0, 0.0, 0.0))
    }
}

impl Component for Velocity { }
impl PillTypeMapKey for Velocity {
    type Storage = ComponentStorage<Self>;
}

// Utility struct for iterating particles
#[derive(Clone, Copy)]
pub struct Pv {
    p: Vector3f,
    v: Vector3f,
}

// ---- Simulation ----
#[derive(Clone)]
pub struct SimulationParameters {
    pub acceleration: f32, // K - how fast particles chase the flow
    pub linear_drag: f32,
    pub amplitude: f32,    // amplitude of the curl field
    pub frequency: f32,    // spatial frequency (cycles/world unit)
    pub time_scale: f32,   // how fast field morphs
    pub eps: f32,          // finite difference step in sample space
    // for fBm noise
    //pub octaves: u32,     // number of curl octaves
    //pub lacunarity: f32,  // frequency multiplier per octave
    //pub gain: f32,        // amplitude multiplier per octave
    //pub turb_mult: f32, //  extra-high freq multiplier for turbulence
    //pub turb_amp: f32,  // small extra amplitude for turbulence band
    //pub wrap_bounds: bool // switch to wrapping vs reflection
}

impl Default for SimulationParameters {
    fn default() -> Self {
        Self {
            acceleration: 10.0,
            linear_drag: 1.0,
            amplitude: 8.0,
            frequency: 0.4, // 0.4 cycles/unit =>  ~8 cycles across a 20-unit box
            time_scale: 0.2,
            eps: 0.12, // 1e-3..1e-2
            // fBm:
            //octaves: 3,
            //lacunarity: 2.0,
            //gain: 0.5,
            //turb_mult: 8.0,
            //turb_amp: 2.0,
            //wrap_bounds: false,
        }
    }
}

impl GlobalComponent for SimulationParameters {}
impl PillTypeMapKey for SimulationParameters {
    type Storage = GlobalComponentStorage<Self>;
}

#[derive(Clone, Copy)]
pub struct AABB {
    pub min: Vector3f,
    pub max: Vector3f,
    pub restitution: f32, // 0..1
    pub friction: f32,  // tangential slow-down on bounce
}

impl Default for AABB {
    fn default() -> Self {
        Self {
            min: Vector3f::new(-10.0, -10.0, -10.0),
            max: Vector3f::new(10.0, 10.0, 10.0),
            restitution: 0.6,
            friction: 0.95,
        }
    }
}

impl GlobalComponent for AABB {}
impl PillTypeMapKey for AABB {
    type Storage = GlobalComponentStorage<Self>;
}

#[derive(Clone)]
pub struct VelocityGrid {
    pub origin: Vector3f,
    pub cell: f32,
    pub inv_cell: f32,
    pub size: [u32; 3],
    pub sx: i32,  // 1
    pub sy: i32, // nx
    pub sz: i32, // nx * ny
    pub data: Arc<[Vector3f]>,
}

impl GlobalComponent for VelocityGrid {}
impl PillTypeMapKey for VelocityGrid {
    type Storage = GlobalComponentStorage<Self>;
}

#[derive(Clone)]
pub struct GridState {
    pub a: VelocityGrid,
    pub b: VelocityGrid,
    pub blended: VelocityGrid,
    pub last_rebuild_time: f32,
    pub period: f32,
    pub t_mix: f32,
}

impl GlobalComponent for GridState {}
impl PillTypeMapKey for GridState {
    type Storage = GlobalComponentStorage<Self>;
}

fn make_velocity_grid(aabb: AABB, nx: u32, ny: u32, nz: u32) -> VelocityGrid {
    let extent = aabb.max - aabb.min;
    let cell_x = extent.x / nx as f32;
    let cell_y = extent.y / ny as f32;
    let cell_z = extent.z / nz as f32;

    // uniform cell - cubic
    let cell = cell_x.min(cell_y).min(cell_z);
    let len = (nx * ny * nz) as usize;
    let vec = vec![Vector3f::zero(); len];

    VelocityGrid {
        origin: aabb.min,
        cell,
        inv_cell: 1.0 / cell,
        size: [nx, ny, nz],
        sx: 1,
        sy: nx as i32,
        sz: (nx * ny) as i32,
        data: Arc::<[Vector3f]>::from(vec.into_boxed_slice()),
    }
}

fn grid_rebuild(g: &mut VelocityGrid, curl: &CurlField, sp: &SimulationParameters, t: f32) -> Result<()> {
    let data: &mut [Vector3f] = Arc::make_mut(&mut g.data);
    let [nx, ny, nz] = g.size;
    for z in 0..nz {
        for y in 0..ny {
            for x in 0..nx {
                let p = g.origin + Vector3f::new(x as f32, y as f32, z as f32) * g.cell;
                data[(z * ny * nx + y * nx + x) as usize] = curl.curl_at(p, t, &sp);
            }
        }
    }
    Ok(())
}

fn grid_rebuild_fast_par(g: &mut VelocityGrid, curl: &CurlField, sp: &SimulationParameters, t: f32) ->  Result<()> {
    let [nxu, nyu, nzu] = g.size;
    let nx = nxu as usize;
    let ny = nyu as usize;
    let nz = nzu as usize;

    let cell = g.cell;
    let inv2h_s = 1.0 / (2.0 * cell * sp.frequency);
    let plane = nx * ny;

    let out: &mut [Vector3f] = Arc::make_mut(&mut g.data);
    let mut fvals = vec![Vector3f::zero(); nx * ny * nz];

    let st = t * sp.time_scale;
    let sxs: Vec<f32> = (0..nx).map(|x| (g.origin.x + x as f32 * cell) * sp.frequency).collect();
    let sys: Vec<f32> = (0..ny).map(|y| (g.origin.y + y as f32 * cell) * sp.frequency).collect();
    let szs: Vec<f32> = (0..nz).map(|z| (g.origin.z + z as f32 * cell) * sp.frequency).collect();

    // Compute F at all grid points
    fvals.par_chunks_mut(plane).enumerate().for_each(|(z, slab)| {
        let sz = szs[z];
        for y in 0..ny {
            let sy = sys[y];
            let row = &mut slab[y * nx..(y + 1) * nx];
            for x in 0..nx {
                let sx = sxs[x];
                row[x] = curl.f_sample(sx, sy, sz, st);
            }
        }
    });

    // Immmutable view for curl computation
    let fvals_ref = &fvals;

    // Curl via finite differences
    // TODO: refactor so we have clear responsibilities once it work fast
    out.par_chunks_mut(plane).enumerate().for_each(|(z, slab_out)| {
        let zm = wrap(z as i32 - 1, nz);
        let zp = wrap(z as i32 + 1, nz);
        for y in 0..ny {
            let ym = wrap(y as i32 - 1, ny);
            let yp = wrap(y as i32 + 1, ny);
            for x in 0..nx {
                let xm = wrap(x as i32 - 1, nx);
                let xp = wrap(x as i32 + 1, nx);

                let dfdx = (fvals_ref[idx(nx, ny, xp, y, z)] - fvals_ref[idx(nx, ny, xm, y, z)]) * inv2h_s;
                let dfdy = (fvals_ref[idx(nx, ny, x, yp, z)] - fvals_ref[idx(nx, ny, x, ym, z)]) * inv2h_s;
                let dfdz = (fvals_ref[idx(nx, ny, x, y, zp)] - fvals_ref[idx(nx, ny, x, y, zm)]) * inv2h_s;

                slab_out[y * nx + x] = Vector3f::new(
                    dfdy.z - dfdz.y,
                    dfdz.x - dfdx.z,
                    dfdx.y - dfdy.x,
                ) * sp.amplitude;
            }
        }
    });

    Ok(())
}

#[inline]
fn grid_sample(g: &VelocityGrid, p: Vector3f) -> Vector3f {
    // remap from world to grid cell space
    let rel = (p - g.origin) * g.inv_cell;
    // split into integer and fractional parts
    let ix = rel.x.floor() as i32; let fx = rel.x - ix as f32;
    let iy = rel.y.floor() as i32; let fy = rel.y - iy as f32;
    let iz = rel.z.floor() as i32; let fz = rel.z - iz as f32;

    let nx = g.size[0] as i32; let ny = g.size[1] as i32; let nz = g.size[2] as i32;

    // Fast in-bound path (common case)
    if ix >= 0 && iy >= 0 && iz >= 0 && ix + 1 < nx && iy + 1 < ny && iz + 1 < nz {
        let base = (iz * g.sz + iy * g.sy + ix) as usize;

        // load 8 neighbors via strides
        let v000 = g.data[base];
        let v100 = g.data[(base + g.sx as usize)];
        let v010 = g.data[(base + g.sy as usize)];
        let v110 = g.data[(base + g.sy as usize + g.sx as usize)];
        let v001 = g.data[(base + g.sz as usize)];
        let v101 = g.data[(base + g.sz as usize + g.sx as usize)];
        let v011 = g.data[(base + g.sz as usize + g.sy as usize)];
        let v111 = g.data[(base + g.sz as usize + g.sy as usize + g.sx as usize)];

        // trilinear (no closures; use mul_add where your Vector3f supports it)
        let vx00 = v000 + (v100 - v000) * fx;
        let vx10 = v010 + (v110 - v010) * fx;
        let vx01 = v001 + (v101 - v001) * fx;
        let vx11 = v011 + (v111 - v011) * fx;

        let vxy0 = vx00 + (vx10 - vx00) * fy;
        let vxy1 = vx01 + (vx11 - vx01) * fy;

        return vxy0 + (vxy1 - vxy0) * fz;
    }

    // Slow wrap path (rare)
    let x0 = wrap(ix, nx as usize) as usize; let x1 = wrap(ix + 1, nx as usize) as usize;
    let y0 = wrap(iy, ny as usize) as usize; let y1 = wrap(iy + 1, ny as usize) as usize;
    let z0 = wrap(iz, nz as usize) as usize; let z1 = wrap(iz + 1, nz as usize) as usize;

    let v000 = g.data[idx(nx as usize, ny as usize, x0,y0,z0)];
    let v100 = g.data[idx(nx as usize, ny as usize, x1,y0,z0)];
    let v010 = g.data[idx(nx as usize, ny as usize, x0,y1,z0)];
    let v110 = g.data[idx(nx as usize, ny as usize, x1,y1,z0)];
    let v001 = g.data[idx(nx as usize, ny as usize, x0,y0,z1)];
    let v101 = g.data[idx(nx as usize, ny as usize, x1,y0,z1)];
    let v011 = g.data[idx(nx as usize, ny as usize, x0,y1,z1)];
    let v111 = g.data[idx(nx as usize, ny as usize, x1,y1,z1)];

    let vx00 = v000 + (v100 - v000) * fx;
    let vx10 = v010 + (v110 - v010) * fx;
    let vx01 = v001 + (v101 - v001) * fx;
    let vx11 = v011 + (v111 - v011) * fx;

    let vxy0 = vx00 + (vx10 - vx00) * fy;
    let vxy1 = vx01 + (vx11 - vx01) * fy;

    vxy0 + (vxy1 - vxy0) * fz
}

#[inline]
fn pre_blend_grids(g: &mut GridState) {
    let out: &mut [Vector3f] = Arc::make_mut(&mut g.blended.data);
    let a =  &g.a.data;
    let b = &g.b.data;
    let tm = g.t_mix;
    out.par_iter_mut().zip(a.par_iter().zip(b.par_iter())).for_each(|(o, (va, vb))| *o = *va * (1.0 - tm) + *vb * tm);
}

// ---- Curl field implementation ----
#[derive(Clone)]
pub struct CurlField {
    noise: OpenSimplex,
    // decorrelation offsets for each component
    offset_x: Vector3f,
    offset_y: Vector3f,
    offset_z: Vector3f,
    tau_x: f32,
    tau_y: f32,
    tau_z: f32,
}

impl GlobalComponent for CurlField {}
impl PillTypeMapKey for CurlField {
    type Storage = GlobalComponentStorage<Self>;
}

impl CurlField {
    pub fn new(seed: u32) -> Self {
        Self {
            noise: OpenSimplex::new(seed),
            offset_x: Vector3f::new(19.31,  -7.13, 11.79),
            offset_y: Vector3f::new(-3.77, 33.71, -21.47),
            offset_z: Vector3f::new(14.53,  9.91, -8.29),
            tau_x: 1.0,
            tau_y: 0.93,
            tau_z: 1.07,
        }
    }

    /// Evaluate F(sx, sy, sz, t) in *sample* space.
    /// Returns a 3D vector field value
    #[inline]
    fn f_sample(&self, sx: f32, sy: f32, sz: f32, st: f32) -> Vector3f {
        // 4D OpenSimplex noise per component; time is fourth dimension
        let fx = self.noise.get([sx as f64 + self.offset_x.x as f64,
                                      sy as f64 + self.offset_x.y as f64,
                                      sz as f64 + self.offset_x.z as f64,
                                      st as f64 * self.tau_x as f64]) as f32;

        let fy = self.noise.get([sx as f64 + self.offset_y.x as f64,
                                      sy as f64 + self.offset_y.y as f64,
                                      sz as f64 + self.offset_y.z as f64,
                                      st as f64 * self.tau_y as f64]) as f32;

        let fz = self.noise.get([sx as f64 + self.offset_z.x as f64,
                                        sy as f64 + self.offset_z.y as f64,
                                        sz as f64 + self.offset_z.z as f64,
                                        st as f64 * self.tau_z as f64]) as f32;
        Vector3f::new(fx, fy, fz)
    }

    /// Compute curl(F) at world position p and time t.
    /// Uses forward differences with step eps in sample space.
    /// Chain rule multiplies by frequency.
    /// Finally scale by amplitude.
    #[inline]
    pub fn curl_at(&self, p: Vector3f, t: f32, parameters: &SimulationParameters) -> Vector3f {
        let sx = p.x * parameters.frequency;
        let sy = p.y * parameters.frequency;
        let sz = p.z * parameters.frequency;
        let st = t * parameters.time_scale;
        let e = parameters.eps;

        // F at +/- sample steps
        let f0 = self.f_sample(sx, sy, sz, st);
        let fx1 = self.f_sample(sx + e, sy, sz, st);
        let fy1 = self.f_sample(sx, sy + e, sz, st);
        let fz1 = self.f_sample(sx, sy, sz + e, st);

        // sample-space partial dF/ds*
        let dFdx = (fx1 - f0) * (1.0 / e) * parameters.frequency;
        let dFdy = (fy1 - f0) * (1.0 / e) * parameters.frequency;
        let dFdz = (fz1 - f0) * (1.0 / e) * parameters.frequency;

        // curl(F) = (d/dy Fz - d/dz Fy, d/dz Fx - d/dx Fz, d/dx Fy - d/dy Fx)
        // Note: dFdx, dFdy, dFdz are vectors holding partials of all components
        let curl = Vector3f::new(
            dFdy.z - dFdz.y,
            dFdz.x - dFdx.z,
            dFdx.y - dFdy.x,
        );

        curl * parameters.amplitude
    }
}

pub fn particle_spawn_oneshot(engine: &mut Engine) -> Result<()> {
    let mut rng = rand::rng();
    let scene = engine.get_active_scene_handle()?;
    let aabb = engine.get_global_component::<AABB>()?.clone();
    // Add a MeshRenderingComponent to visualize the Particle
    // TODO: change it to a different mesh/material
    let particle_mesh_handle = engine.get_resource_handle::<Mesh>("pill")?;
    let particle_material_handle = engine.get_resource_handle::<Material>("pill")?;

    for _ in 0..PARTICLE_COUNT{
        let particle = engine.create_entity(scene)?;
        let position = Vector3f::new(
            rng.random_range(aabb.min.x..aabb.max.x),
            rng.random_range(aabb.min.y..aabb.max.y),
            rng.random_range(aabb.min.z..aabb.max.z),
        );
        let transform = TransformComponent::builder()
            .position(position)
            .scale(Vector3f::new(0.1, 0.1, 0.1))
            .build();
        engine.add_component_to_entity(scene, particle, transform)?;
        engine.add_component_to_entity(scene, particle, Particle {})?;
        engine.add_component_to_entity(scene, particle, Velocity::default())?;

        let mesh_rendering_component = MeshRenderingComponent::builder()
            .mesh(&particle_mesh_handle)
            .material(&particle_material_handle)
            .build();

        engine.add_component_to_entity(scene, particle, mesh_rendering_component)?;
    }

    Ok(())
}

pub fn curl_integration_system(engine: &mut Engine) -> Result<()> {
    let (sp, mut dt, blended_grid, aabb) = {
        let sp = engine.get_global_component::<SimulationParameters>()?.clone();
        let time = engine.get_global_component::<TimeComponent>()?;
        let blended_grid = engine.get_global_component::<GridState>()?.blended.clone();
        let aabb = engine.get_global_component::<AABB>()?.clone();
        (sp, time.delta_time, blended_grid, aabb)
    };

    if dt > 1.0/30.0 { dt = 1.0/30.0; } // avoid large steps

    let alpha = 1.0 - (-sp.acceleration * dt).exp(); // in [0,1)
    let drag = (-sp.linear_drag * dt).exp();

    engine.par_for_each2_with::<TransformComponent, Velocity, Particle, _>(512, |t, v| {
        let mut p = t.position;
        // flow velocity from curl(F)
        let u: Vector3f = grid_sample(&blended_grid, p);
        // frame-rate independent blend towards u
        v.0 += (u - v.0) * alpha;
        // frame-rate independent linear drag
        v.0 *= drag;
        p += v.0 * dt;
        respect_aabb_bounds(&mut p, &mut v.0, &aabb);
        t.set_position(p);
    });

    Ok(())
}

pub fn grid_update_system(engine: &mut Engine) -> Result<()> {
    let curl = engine.get_global_component::<CurlField>()?.clone();
    let sp = engine.get_global_component::<SimulationParameters>()?.clone();
    let time = engine.get_global_component::<TimeComponent>()?.time;

    // check if it's time to rebuild
    {
        let gs = engine.get_global_component_mut::<GridState>()?;

        let elapsed = time - gs.last_rebuild_time;
        if elapsed >= gs.period {
            // swap grids
            std::mem::swap(&mut gs.a, &mut gs.b);
            // rebuild the new "b" grid
            grid_rebuild_fast_par(&mut gs.b, &curl, &sp, time)?;

            gs.last_rebuild_time = time;
            gs.t_mix = 0.0;
            pre_blend_grids(gs);
        } else {
            // otherwise just update interpolation parameter
            gs.t_mix = (elapsed / gs.period).clamp(0.0, 1.0);
        }
    }

    Ok(())
}

pub fn respect_aabb_bounds(p: &mut Vector3f, v: &mut Vector3f, aabb: &AABB) {
    // X
    if p.x < aabb.min.x {
        p.x = aabb.min.x;
        if v.x < 0.0 {
            v.x = -v.x * aabb.restitution;
            v.y *= aabb.friction;
            v.z *= aabb.friction;
        }
    } else if p.x > aabb.max.x {
        p.x = aabb.max.x;
        if v.x > 0.0 {
            v.x = -v.x * aabb.restitution;
            v.y *= aabb.friction;
            v.z *= aabb.friction;
        }
    }

    // Y
    if p.y < aabb.min.y {
        p.y = aabb.min.y;
        if v.y < 0.0 {
            v.y = -v.y * aabb.restitution;
            v.x *= aabb.friction;
            v.z *= aabb.friction;
        }
    } else if p.y > aabb.max.y {
        p.y = aabb.max.y;
        if v.y > 0.0 {
            v.y = -v.y * aabb.restitution;
            v.x *= aabb.friction;
            v.z *= aabb.friction;
        }
    }

    // Z
    if p.z < aabb.min.z {
        p.z = aabb.min.z;
        if v.z < 0.0 {
            v.z = -v.z * aabb.restitution;
            v.x *= aabb.friction;
            v.y *= aabb.friction;
        }
    } else if p.z > aabb.max.z {
        p.z = aabb.max.z;
        if v.z > 0.0 {
            v.z = -v.z * aabb.restitution;
            v.x *= aabb.friction;
            v.y *= aabb.friction;
        }
    }
}

// Game
pub struct Game { }

impl PillGame for Game {
    fn start(&self, engine: &mut Engine) -> Result<()> {
        // Create scene
        let active_scene = engine.create_scene("default")?;
        engine.set_active_scene(active_scene)?;

        // Register components
        engine.register_component::<TransformComponent>(active_scene)?;
        engine.register_component::<MeshRenderingComponent>(active_scene)?;
        engine.register_component::<CameraComponent>(active_scene)?;
        engine.register_component::<AudioListenerComponent>(active_scene)?;
        engine.register_component::<AudioSourceComponent>(active_scene)?;
        engine.register_component::<PillComponent>(active_scene)?;
        engine.register_component::<Particle>(active_scene)?;
        engine.register_component::<Velocity>(active_scene)?;

        engine.add_global_component::<SimulationParameters>(SimulationParameters::default())?;
        engine.add_global_component::<AABB>(AABB::default())?;
        engine.add_global_component::<CurlField>(CurlField::new(0x1234567))?;

        let aabb = engine.get_global_component::<AABB>()?.clone();
        let mut g0 = make_velocity_grid(aabb, 16, 16, 16);
        let mut g1 = make_velocity_grid(aabb, 16, 16, 16);
        // TODO: we don't really need to have it initializes like that
        let blended = make_velocity_grid(aabb, 16, 16, 16);
        // build the grids
        {
            let curl = engine.get_global_component::<CurlField>()?.clone();
            let sp = engine.get_global_component::<SimulationParameters>()?.clone();
            grid_rebuild(&mut g0, &curl, &sp, 0.0)?;
            grid_rebuild(&mut g1, &curl, &sp, 0.0)?;
            engine.add_global_component::<GridState>(GridState {
                a: g0,
                b: g1,
                blended,
                last_rebuild_time: 0.0,
                period: 2.0, // time between grid rebuilds in seconds
                t_mix: 0.0,  // in [0,1], interpolation parameter between a and b
            })?;
            // TODO: observation: we should allow for chaining - add_global_component should return
            // a handle - we might want to immediately use this copmonent
            let gs = engine.get_global_component_mut::<GridState>()?;
            pre_blend_grids(gs);
        }

        // Add systems
        engine.add_system("grid_update", grid_update_system)?;
        engine.add_system("curl_integration", curl_integration_system)?;
        engine.add_system("camera_rotation", camera_rotation_system)?;

        // Add meshes
        let pill_mesh = Mesh::new("pill", "models/pill.obj".into());
        let pill_mesh_handle = engine.add_resource(pill_mesh)?;

        // Add textures
        let pill_color_texture = Texture::new("pill_color", TextureType::Color, ResourceLoadType::Path("textures/pill_color.png".into()));
        let pill_color_texture_handle = engine.add_resource::<Texture>(pill_color_texture)?;
        let pill_normal_texture = Texture::new("pill_normal", TextureType::Normal, ResourceLoadType::Path("textures/pill_normal.png".into()));
        let pill_normal_texture_handle = engine.add_resource::<Texture>(pill_normal_texture)?;

        // Add materials
        let mut pill_material = Material::new("pill");
        pill_material.set_texture("Color", pill_color_texture_handle)?;
        pill_material.set_texture("Normal", pill_normal_texture_handle)?;
        pill_material.set_color("Tint", Color::new( 1.0, 1.0, 1.0))?;
        pill_material.set_scalar("Specularity", 0.5)?; // TODO: names are already corrected upstream
        let pill_material_handle = engine.add_resource::<Material>(pill_material)?;

        // Create camera entity
        let camera = engine.create_entity(active_scene)?;
        let transform_component = TransformComponent::builder()
            .position(Vector3f::new(0.0,0.0,20.0))
            .rotation(Vector3f::new(0.0,0.0,0.0))
            .build();
        engine.add_component_to_entity(active_scene, camera, transform_component)?;
        let camera_component = CameraComponent::builder().enabled(true).build();
        engine.add_component_to_entity(active_scene, camera, camera_component)?;

        // Create pill entity
        let pill = engine.create_entity(active_scene)?;
        let transform_component = TransformComponent::builder()
            .position(Vector3f::new(0.0,-5.0,-5.0))
            .rotation(Vector3f::new(0.0,0.0,0.0))
            .build();
        engine.add_component_to_entity(active_scene, pill, transform_component)?;
        let mesh_rendering_component = MeshRenderingComponent::builder()
            .mesh(&pill_mesh_handle)
            .material(&pill_material_handle)
            .build();
        //engine.add_component_to_entity(active_scene, pill, mesh_rendering_component)?;
        //engine.add_component_to_entity(active_scene, pill, PillComponent {})?;

        particle_spawn_oneshot(engine)?;

        Ok(())
    }
}

fn camera_rotation_system(engine: &mut Engine) -> Result<()> {
    let delta_time = engine.get_global_component::<TimeComponent>()?.delta_time;
    let input_component = engine.get_global_component::<InputComponent>()?;

    // Rotate camera around centre if spacebar is pressed
    if input_component.get_key(KeyboardKey::Space) {
        let angle = 90.0 * delta_time;
        let center = Vector3f::zero();
        let up = Vector3fExt::Y;

        for (_, transform_component, _) in engine.iterate_two_components_mut::<TransformComponent, CameraComponent>()? {
            transform_component.orbit_around_point(center, up, angle);
            transform_component.look_at(center);
        }
    }

    Ok(())
}
