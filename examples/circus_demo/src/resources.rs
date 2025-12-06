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

    let pillars_mesh = Mesh::new("pillars", "models/pillars.obj".into());
    let pillars_mesh_handle = engine.add_resource(pillars_mesh)?;

    let ground_mesh = Mesh::new("ground", "models/ground.obj".into());
    let ground_mesh_handle = engine.add_resource(ground_mesh)?;

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

    let pillars_diffuse_texture = Texture::new(
        "pillars_diffuse",
        TextureType::Gamma,
        ResourceLoadType::Path(
            "textures/pillars/KB3D_SOL_TrimStoneSideElementsAncient_basecolor.png".into(),
        ),
    );
    let pillars_diffuse_texture_handle = engine.add_resource::<Texture>(pillars_diffuse_texture)?;

    let pillars_roughness_texture = Texture::new(
        "pillars_roughness",
        TextureType::Linear,
        ResourceLoadType::Path(
            "textures/pillars/KB3D_SOL_TrimStoneSideElementsAncient_roughness.png".into(),
        ),
    );
    let pillars_roughness_texture_handle =
        engine.add_resource::<Texture>(pillars_roughness_texture)?;

    let pillars_normal_texture = Texture::new(
        "pillars_normal",
        TextureType::Linear,
        ResourceLoadType::Path(
            "textures/pillars/KB3D_SOL_TrimStoneSideElementsAncient_normal.png".into(),
        ),
    );
    let pillars_normal_texture_handle = engine.add_resource::<Texture>(pillars_normal_texture)?;

    let ground_texture_diffuse = Texture::new(
        "ground_diffuse",
        TextureType::Gamma,
        ResourceLoadType::Path("textures/ground/rocks_ground_02_col_4k.jpg".into()),
    );
    let ground_texture_diffuse_handle = engine.add_resource::<Texture>(ground_texture_diffuse)?;

    let ground_texture_normal = Texture::new(
        "ground_normal",
        TextureType::Linear,
        ResourceLoadType::Path("textures/ground/rocks_ground_02_nor_dx_4k.jpg".into()),
    );
    let ground_texture_normal_handle = engine.add_resource::<Texture>(ground_texture_normal)?;

    let ground_texture_roughness = Texture::new(
        "ground_roughness",
        TextureType::Linear,
        ResourceLoadType::Path("textures/ground/rocks_ground_02_rough_4k.jpg".into()),
    );
    let ground_texture_roughness_handle =
        engine.add_resource::<Texture>(ground_texture_roughness)?;

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

    let mut dark_material = PBRMaterial::new("dark");
    dark_material.set_base_color_factor(Color::new(0.8, 0.1, 0.1));
    dark_material.set_emissive_texture(grid_texture_handle);
    dark_material.set_emissive_factor(Color::new(7.0, 1.0, 1.0));
    dark_material.set_albedo_texture(grid_texture_handle);
    dark_material.set_metallic_factor(1.0);
    dark_material.set_roughness_factor(0.3);
    let dark_material_handle = engine.add_resource::<PBRMaterial>(dark_material)?;

    let mut grid_material = PBRMaterial::new("grid");
    grid_material.set_albedo_texture(grid_texture_handle);
    grid_material.set_uv_tiling(60.0, 60.0);
    let grid_material_handle: PBRMaterialHandle =
        engine.add_resource::<PBRMaterial>(grid_material)?;

    let mut wood_material = PBRMaterial::new("wood");
    wood_material.set_albedo_texture(wood_diffuse_texture_handle);
    wood_material.set_normal_texture(wood_normal_texture_handle);
    wood_material.set_metallic_roughness_texture(wood_roughness_texture_handle);
    let wood_material_handle = engine.add_resource::<PBRMaterial>(wood_material)?;

    let mut pillars_material = PBRMaterial::new("pillars");
    pillars_material.set_albedo_texture(pillars_diffuse_texture_handle);
    pillars_material.set_normal_texture(pillars_normal_texture_handle);
    pillars_material.set_metallic_roughness_texture(pillars_roughness_texture_handle);
    pillars_material.set_roughness_factor(0.9);
    let pillars_material_handle = engine.add_resource::<PBRMaterial>(pillars_material)?;

    let mut ground_material = PBRMaterial::new("ground");
    ground_material.set_albedo_texture(ground_texture_diffuse_handle);
    ground_material.set_normal_texture(ground_texture_normal_handle);
    ground_material.set_uv_tiling(3.0, 3.0);
    ground_material.set_roughness_factor(0.9);
    ground_material.set_metallic_roughness_texture(ground_texture_roughness_handle);
    let ground_material_handle = engine.add_resource::<PBRMaterial>(ground_material)?;

    Ok(())
}
