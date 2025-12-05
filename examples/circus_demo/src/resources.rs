use pill_engine::game::*;

pub fn create_resources(engine: &mut Engine) -> Result<()> {
    // ----------- Create meshes -----------
    let pill_mesh = Mesh::new("pill", "models/pill.obj".into());
    let pill_mesh_handle = engine.add_resource(pill_mesh)?;

    let cube_mesh = Mesh::new("cube", "models/pill.obj".into());
    let cube_mesh_handle = engine.add_resource(cube_mesh)?;

    let torus_mesh = Mesh::new("torus", "models/pill.obj".into());
    let torus_mesh_handle = engine.add_resource(torus_mesh)?;

    let plane_mesh = Mesh::new("plane", "models/plane.obj".into());
    let plane_mesh_handle = engine.add_resource(plane_mesh)?;

    // ----------- Create textures -----------

    let fabric_color_texture = Texture::new(
        "fabric_color",
        TextureType::Gamma,
        ResourceLoadType::Path("textures/fabric_color.jpg".into()),
    );
    let fabric_color_texture_handle = engine.add_resource::<Texture>(fabric_color_texture)?;

    let fabric_normal_texture = Texture::new(
        "fabric_normal",
        TextureType::Linear,
        ResourceLoadType::Path("textures/fabric_normal.jpg".into()),
    );
    let fabric_normal_texture_handle = engine.add_resource::<Texture>(fabric_normal_texture)?;

    let stones_color_texture = Texture::new(
        "stones_color",
        TextureType::Gamma,
        ResourceLoadType::Path("textures/stones_color.jpg".into()),
    );
    let stones_color_texture_handle = engine.add_resource::<Texture>(stones_color_texture)?;

    let stones_normal_texture = Texture::new(
        "stones_normal",
        TextureType::Linear,
        ResourceLoadType::Path("textures/stones_normal.jpg".into()),
    );
    let stones_normal_texture_handle = engine.add_resource::<Texture>(stones_normal_texture)?;

    let organic_color_texture = Texture::new(
        "organic_color",
        TextureType::Gamma,
        ResourceLoadType::Path("textures/organic_color.jpg".into()),
    );
    let organic_color_texture_handle = engine.add_resource::<Texture>(organic_color_texture)?;

    let organic_normal_texture = Texture::new(
        "organic_normal",
        TextureType::Linear,
        ResourceLoadType::Path("textures/organic_normal.jpg".into()),
    );
    let organic_normal_texture_handle = engine.add_resource::<Texture>(organic_normal_texture)?;

    let grid_texture = Texture::new(
        "grid",
        TextureType::Gamma,
        ResourceLoadType::Path("textures/grid.png".into()),
    );
    let grid_texture_handle = engine.add_resource::<Texture>(grid_texture)?;

    // Wood
    let wood_diffuse_texture = Texture::new(
        "wood_diffuse",
        TextureType::Gamma,
        ResourceLoadType::Path("textures/wood/wooden_gate_diff_2k.jpg".into()),
    );
    let wood_diffuse_texture_handle = engine.add_resource::<Texture>(wood_diffuse_texture)?;

    let wood_normal_texture = Texture::new(
        "wood_normal",
        TextureType::Linear,
        ResourceLoadType::Path("textures/wood/wooden_gate_nor_dx_2k.jpg".into()),
    );
    let wood_normal_texture_handle = engine.add_resource::<Texture>(wood_normal_texture)?;

    let wood_roughness_texture = Texture::new(
        "wood_roughness",
        TextureType::Linear,
        ResourceLoadType::Path("textures/wood/wooden_gate_rough_2k.jpg".into()),
    );
    let wood_roughness_texture_handle = engine.add_resource::<Texture>(wood_roughness_texture)?;

    // ----------- Create materials -----------

    // Create textured materials
    let mut fabric_material = PBRMaterial::new("fabric");
    fabric_material.set_albedo_texture(fabric_color_texture_handle);
    fabric_material.set_normal_texture(fabric_normal_texture_handle);
    fabric_material.set_base_color_factor(Color::new(1.0, 0.1, 0.1));
    fabric_material.set_metallic_factor(0.0);
    fabric_material.set_roughness_factor(0.8);
    let fabric_material_handle = engine.add_resource::<PBRMaterial>(fabric_material)?;

    let mut stones_material = PBRMaterial::new("stones");
    stones_material.set_albedo_texture(stones_color_texture_handle);
    stones_material.set_normal_texture(stones_normal_texture_handle);
    stones_material.set_base_color_factor(Color::new(1.0, 1.0, 1.0));
    stones_material.set_metallic_factor(0.0);
    stones_material.set_roughness_factor(0.9);
    let stones_material_handle = engine.add_resource::<PBRMaterial>(stones_material)?;

    let mut organic_material = PBRMaterial::new("organic");
    organic_material.set_albedo_texture(organic_color_texture_handle);
    organic_material.set_normal_texture(organic_normal_texture_handle);
    organic_material.set_base_color_factor(Color::new(0.26, 0.87, 0.9));
    organic_material.set_metallic_factor(0.1);
    organic_material.set_roughness_factor(0.5);
    let organic_material_handle = engine.add_resource::<PBRMaterial>(organic_material)?;

    // Create plain color materials
    let mut yellow_material = PBRMaterial::new("yellow");
    yellow_material.set_base_color_factor(Color::new(1.0, 0.88, 0.0));
    yellow_material.set_metallic_factor(0.0);
    yellow_material.set_roughness_factor(0.7);
    let yellow_material_handle = engine.add_resource::<PBRMaterial>(yellow_material)?;

    let mut blue_material = PBRMaterial::new("blue");
    blue_material.set_base_color_factor(Color::new(0.26, 0.87, 0.9));
    blue_material.set_metallic_factor(0.0);
    blue_material.set_roughness_factor(0.7);
    let blue_material_handle = engine.add_resource::<PBRMaterial>(blue_material)?;

    let mut white_material = PBRMaterial::new("white");
    white_material.set_base_color_factor(Color::new(1.0, 1.0, 1.0));
    white_material.set_metallic_factor(0.0);
    white_material.set_roughness_factor(0.7);
    let white_material_handle = engine.add_resource::<PBRMaterial>(white_material)?;

    let mut grid_material = PBRMaterial::new("grid");
    grid_material.set_albedo_texture(grid_texture_handle);
    grid_material.set_uv_tiling(40.0, 40.0);
    let grid_material_handle: PBRMaterialHandle =
        engine.add_resource::<PBRMaterial>(grid_material)?;

    let mut wood_material = PBRMaterial::new("wood");
    wood_material.set_albedo_texture(wood_diffuse_texture_handle);
    wood_material.set_normal_texture(wood_normal_texture_handle);
    wood_material.set_metallic_roughness_texture(wood_roughness_texture_handle);
    let wood_material_handle = engine.add_resource::<PBRMaterial>(wood_material)?;

    Ok(())
}
