use pill_engine::game::*;
use noise::{NoiseFn, OpenSimplex};
use rand::Rng;
use std::sync::Arc;

const PARTICLE_COUNT: usize = 2000;

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
            time_scale: 0.0,
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
    pub size: [u32; 3],
    pub data: Arc<[Vector3f]>,
}

impl GlobalComponent for VelocityGrid {}
impl PillTypeMapKey for VelocityGrid {
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
        size: [nx, ny, nz],
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

#[inline]
fn grid_sample(g: &VelocityGrid, p: Vector3f) -> Vector3f {
    // remap from world to grid cell space
    let rel = (p - g.origin) / g.cell;
    // split into integer and fractional parts
    let [nx, ny, nz] = g.size;
    let ix = rel.x.floor() as i32; let fx = (rel.x - ix as f32).clamp(0.0, 1.0);
    let iy = rel.y.floor() as i32; let fy = (rel.y - iy as f32).clamp(0.0, 1.0);
    let iz = rel.z.floor() as i32; let fz = (rel.z - iz as f32).clamp(0.0, 1.0);

    // wrap indices so sampling is periodic (toroidal)
    let wrap = |i, n| ((i % n as i32) + n as i32) as u32 % n;
    let x0 = wrap(ix, nx); let x1 = wrap(ix + 1, nx);
    let y0 = wrap(iy, ny); let y1 = wrap(iy + 1, ny);
    let z0 = wrap(iz, nz); let z1 = wrap(iz + 1, nz);

    // convert (x,y,z) to a linear array index
    let idx = |x: u32, y: u32, z: u32| (z * ny * nx + y * nx + x) as usize;

    // read the indices
    let v000 = g.data[idx(x0, y0, z0)];
    let v100 = g.data[idx(x1, y0, z0)];
    let v010 = g.data[idx(x0, y1, z0)];
    let v110 = g.data[idx(x1, y1, z0)];
    let v001 = g.data[idx(x0, y0, z1)];
    let v101 = g.data[idx(x1, y0, z1)];
    let v011 = g.data[idx(x0, y1, z1)];
    let v111 = g.data[idx(x1, y1, z1)];

    // trilinear interpolation
    // lerp along x at z0 and z1 planes
    let lerp = |a: Vector3f, b: Vector3f, t: f32| a + (b - a) * t;
    let vx00 = lerp(v000, v100, fx);
    let vx10 = lerp(v010, v110, fx);
    let vx01 = lerp(v001, v101, fx);
    let vx11 = lerp(v011, v111, fx);

    let vxy0 = lerp(vx00, vx10, fy);
    let vxy1 = lerp(vx01, vx11, fy);

    lerp(vxy0, vxy1, fz)
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

        // Add a MeshRenderingComponent to visualize the Particle
        // TODO: change it to a different mesh/material
        let particle_mesh_handle = engine.get_resource_handle::<Mesh>("pill")?;
        let particle_material_handle = engine.get_resource_handle::<Material>("pill")?;

        let mesh_rendering_component = MeshRenderingComponent::builder()
            .mesh(&particle_mesh_handle)
            .material(&particle_material_handle)
            .build();
        engine.add_component_to_entity(scene, particle, mesh_rendering_component)?;
    }

    Ok(())
}

pub fn curl_integration_system(engine: &mut Engine) -> Result<()> {
    let (curl, sp, mut dt, t) = {
        let curl = engine.get_global_component::<CurlField>()?.clone();
        let sp = engine.get_global_component::<SimulationParameters>()?.clone();
        let time = engine.get_global_component::<TimeComponent>()?;
        (curl, sp, time.delta_time, time.time)
    };

    if dt > 1.0/30.0 { dt = 1.0/30.0; } // avoid large steps

    let alpha = 1.0 - (-sp.acceleration * dt).exp(); // in [0,1)
    let drag = (-sp.linear_drag * dt).exp();

    // Don' rebuild if time_scale is zero
    if sp.time_scale != 0.0 {
        let g = engine.get_global_component_mut::<VelocityGrid>()?;
        grid_rebuild(g, &curl, &sp, t)?; // update velocity grid
    }

    // Arc Clone
    let grid = engine.get_global_component::<VelocityGrid>()?.clone();
    for (_, transform, velocity, _) in engine.iterate_three_components_mut::<TransformComponent, Velocity, Particle>()? {
        // flow velocity from curl(F)
        let p = transform.position;
        let u: Vector3f = grid_sample(&grid, p);

        // frame-rate independent blend towards u
        velocity.0 += (u - velocity.0) * alpha;

        // frame-rate independent linear drag
        velocity.0 *= drag;

        transform.set_position(p + velocity.0 * dt);
    }
    Ok(())
}

pub fn respect_aabb_bounds_system(engine: &mut Engine) -> Result<()> {
    let aabb = engine.get_global_component::<AABB>()?.clone();
    for (_, transform, velocity, _) in engine.iterate_three_components_mut::<TransformComponent, Velocity, Particle>()? {
        let mut p = transform.position;
        let mut v = velocity.0;

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
        transform.set_position(p);
        velocity.0 = v
    }
    Ok(())
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
        engine.add_global_component::<VelocityGrid>(make_velocity_grid(aabb, 16, 16, 16))?;
        // build the grid
        {
            let curl = engine.get_global_component::<CurlField>()?.clone();
            let sp = engine.get_global_component::<SimulationParameters>()?.clone();
            let mut g = engine.get_global_component_mut::<VelocityGrid>()?;
            grid_rebuild(&mut g, &curl, &sp, 0.0)?;
        }

        // Add systems
        engine.add_system("curl_integration", curl_integration_system)?;
        engine.add_system("respect_aabb_bounds", respect_aabb_bounds_system)?;
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
        pill_material.set_texture("color", pill_color_texture_handle)?;
        pill_material.set_texture("normal", pill_normal_texture_handle)?;
        pill_material.set_color("tint", Color::new( 1.0, 1.0, 1.0))?;
        pill_material.set_scalar("specularity", 0.5)?;
        let pill_material_handle = engine.add_resource::<Material>(pill_material)?;

        // Create camera entity
        let camera = engine.create_entity(active_scene)?;
        let transform_component = TransformComponent::builder()
            .position(Vector3f::new(0.0,0.0,-20.0))
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
