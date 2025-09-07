use crate::{
    config::DEFAULT_MATERIAL_HANDLE, 
    ecs::{ components::volume::Volume3D, Component, ComponentStorage, DeferredOperationManagerPointer, EntityHandle, SceneHandle }, 
    engine::Engine, 
    graphics::{ compose_render_queue_key, PostprocessingEffect, RenderQueueKey }, 
    internal::MaterialParameter, 
    resources::{ Material, MaterialHandle, Mesh, MeshHandle, ResourceManager }
};
use pill_core::{
    get_type_name, impl_trait_accessible, BoundingBox, Color, EngineError, OptionStorage, PillTraitTypeMap, PillTypeMap, PillTypeMapKey, SingleStorage, TraitAccessible, TraitAccessor, Vector2f, Vector3f
};
use anyhow::{Context, Result, Error};
use std::{ 
    any::Any, 
    cell::RefCell, 
    collections::{ HashMap, VecDeque }, 
    marker::PhantomData, 
    ops::IndexMut
};



// #[derive(Debug, PartialEq, Clone, Copy)]
// pub enum PostprocessingEffectType {
//     // Color adjustments
//     ColorAdjustment,
//     // BrightnessContrast,
//     // HueSaturation,
//     // ColorGrading,
//     // ColorInversion,
//     // ToneMapping,
//     // Exposure,
//     // WhiteBalance,
//     // Gamma,

//     // Effects
//     Bloom,
//     ChromaticAberration,
//     Vignette,
//     DepthOfField,
//     FilmGrain,
//     LensDistortion,
//     Sharpen,
//     AmbientOcclusion,
//     Fog,

//     // Blur effects
//     GaussianBlur,
//     MotionBlur,
//     TiltShift,
// }

// Register postprocessing effects
// shader, parameters -> create material -> store material handle in effect


// #[readonly::make]
// pub struct Effect {
//     #[readonly]
//     pub name: String,
//     material: MaterialHandle,
//     enabled: bool,
//     opacity: f32,
//     #[readonly]
//     pub parameters: HashMap<String, MaterialParameter>,
// }


		// Each postprocess effect type should have its own material
		// In postprocess volume component we should store handle to this material
		// We shouldn't be able to get this material from resource manager directly
		// We should be able to add effect to the volume. This means that effect should be a struct or enum.

		// Struct is nicer because we can have all parameters.

		// add_custom_effect(name, shader_handle, parameters) -> create material -> store: name -> (material_handle, parameters)


		// struct VignetteEffect {
		//     intensity: f32,
		//     smoothness: f32,
		//     roundness: f32,
		//     center: (f32, f32)
		// }

		// impl PostprocessEffect for VignetteEffect {
		//     fn get_material_handle(&self, engine: &Engine) -> Result<MaterialHandle> {
		//         let material_handle = engine.get_resource_handle::<Material>("pill_engine_vignette_shader")?;
		//         Ok(material_handle)
		//     }
		//
		//     fn get_parameters(&self) -> HashMap<String, MaterialParameter> {
		//         let mut parameters = HashMap::new();
		//         parameters.insert("intensity".to_string(), MaterialParameter::Scalar(self.intensity));
		//         parameters.insert("smoothness".to_string(), MaterialParameter::Scalar(self.smoothness));
		//         parameters.insert("roundness".to_string(), MaterialParameter::Scalar(self.roundness));
		//         parameters.insert("center".to_string(), MaterialParameter::Vector2(self.center));
		//         parameters
		//     }
		// }


		
		// let vignette_effect = VignetteEffect {
		//     intensity: 0.5,
		//     smoothness: 0.5,
		//     roundness: 0.5,
		//     center: (0.5, 0.5)
		// };

		// let mut postprocess_volume = PostprocessingVolumeComponent::new();
		// postprocess_volume.add_effect::<Vignette>(vignette_effect)?;


		// let vignette_effect = CustomVignetteEffect {
		//     intensity: 0.5,
		//     smoothness: 0.5,
		//     roundness: 0.5,
		//     center: (0.5, 0.5)
		// };

		// postprocess_volume.add_custom_effect::<CustomVignette>(vignette_effect)?;


// --- Builder ---

pub struct PostprocessingVolumeComponentBuilder {
    component: PostprocessingVolumeComponent,
}

impl PostprocessingVolumeComponentBuilder {
    pub fn default() -> Self {
        Self {
            component: PostprocessingVolumeComponent::new(),
        }
    }

    pub fn is_enabled(mut self, is_enabled: bool) -> Self {
        self.component.is_enabled = is_enabled;
        self
    }

    pub fn is_global(mut self, is_global: bool) -> Self {
        self.component.is_global = is_global;
        self
    }

    pub fn falloff(mut self, falloff: f32) -> Self {
        self.component.falloff = falloff;
        self
    }

    pub fn bounding_box(mut self, bounding_box: BoundingBox) -> Self 
    {
        self.component.bounding_box = bounding_box;
        self
    }
    
    pub fn add_effect<T>(mut self, effect: T) -> Result<Self>
        where T: PostprocessingEffect + TraitAccessible<dyn PostprocessingEffect>
    {
        self.component.add_effect(effect)?;
        Ok(self)
    }

    pub fn build(self) -> PostprocessingVolumeComponent {
        self.component
    }
}

#[readonly::make]
pub struct PostprocessingVolumeComponent {
    pub is_enabled: bool,
    pub is_global: bool,
    pub bounding_box: BoundingBox,
    pub falloff: f32,
    pub effects: PillTraitTypeMap<dyn PostprocessingEffect, SingleStorage>,

    entity_handle: Option<EntityHandle>,
    scene_handle: Option<SceneHandle>,
    deferred_operation_manager: Option<DeferredOperationManagerPointer>,
}

impl PostprocessingVolumeComponent {
    pub fn builder() -> PostprocessingVolumeComponentBuilder {
        PostprocessingVolumeComponentBuilder::default()
    }

    pub fn new() -> Self {
        Self {
            is_enabled: true,
            is_global: false,
            bounding_box: BoundingBox::new(Vector3f::new(-1.0, -1.0, -1.0), Vector3f::new(1.0, 1.0, 1.0)),
            falloff: 0.0,
            effects: PillTraitTypeMap::new(),
            entity_handle: None,
            scene_handle: None,
            deferred_operation_manager: None,
        }
    }

    pub fn get_effect<T>(&self) -> Result<&T> 
        where T: PostprocessingEffect + TraitAccessible<dyn PostprocessingEffect>
    {
        let effect_storage = self.effects.get_storage::<T>()
            .map_err(|_| EngineError::PostprocessingEffectNotFound(get_type_name::<T>()))?;

        let effect = effect_storage.data.as_ref()
            .ok_or(EngineError::PostprocessingEffectNotFound(get_type_name::<T>()))?;

        Ok(effect)
    }

    pub fn get_effect_mut<T>(&mut self) -> Result<&mut T> 
        where T: PostprocessingEffect + TraitAccessible<dyn PostprocessingEffect>
    {
        let effect_storage = self.effects.get_storage_mut::<T>()
            .map_err(|_| EngineError::PostprocessingEffectNotFound(get_type_name::<T>()))?;

        let effect = effect_storage.data.as_mut()
            .ok_or(EngineError::PostprocessingEffectNotFound(get_type_name::<T>()))?;

        Ok(effect)
    }

    pub fn add_effect<T>(&mut self, effect: T) -> Result<()> 
        where T: PostprocessingEffect + TraitAccessible<dyn PostprocessingEffect>
    {
        // Register type storage if not already registered
        if !self.effects.is_type_storage_registered::<T>() {
            let _ = self.effects.register_type_storage::<T>();
        }

        // Get mutable storage for the effect type
        let effect_storage = self.effects.get_storage_mut::<T>()
            .map_err(|_| EngineError::PostprocessingEffectNotFound(get_type_name::<T>()))?;

        // Check if effect already exists
        if effect_storage.data.is_some() {
            return Err(EngineError::PostprocessingEffectAlreadyExists(get_type_name::<T>()).into());
        }

        // Store the effect
        effect_storage.data = Some(effect);

        // Deffered update.
        // if not custom effect then find material for it and assign handle

        
        // Each effect should have setter.
        // Setting should update material. 

        // Each effect should have map of parameter slots to parameters.

        // We find overlapping volumes and pass them to the renderer in render method.
        // In postprocessing render pass we iterate over volumes
        // we get interate their effects and get each renderer material handles.
        // from each effect we get parameters: &HashMap<String, MaterialParameter>
        // this is enough to write_parameters_to_buffer
        // - simply call material.update_parameters


        
        Ok(())
    }

    pub fn remove_effect<T>(&mut self) -> Result<()> 
        where T: PostprocessingEffect + TraitAccessible<dyn PostprocessingEffect>
    {
        // Get mutable storage for the effect type
        let effect_storage = self.effects.get_storage_mut::<T>()
            .map_err(|_| EngineError::PostprocessingEffectNotFound(get_type_name::<T>()))?;

        // Check if effect exists
        if effect_storage.data.is_none() {
            return Err(EngineError::PostprocessingEffectNotFound(get_type_name::<T>()).into());
        }

        // Remove the effect
        effect_storage.data = None;

        Ok(())
    }
}

impl Volume3D for PostprocessingVolumeComponent {
    fn set_is_global(&mut self, is_global: bool) {
        self.is_global = is_global;
    }

    fn is_global(&self) -> bool {
        self.is_global
    }

    fn get_bounding_box(&self) -> Option<BoundingBox> {
        if self.is_global {
            return None;
        }
        
        Some(self.bounding_box)
    }
}

impl PillTypeMapKey for PostprocessingVolumeComponent {
    type Storage = ComponentStorage<PostprocessingVolumeComponent>; 
}

impl Component for PostprocessingVolumeComponent {
//     fn initialize(&mut self, engine: &mut Engine) -> Result<()> {
//         // This component is using DeferredOperationSystem so keep DeferredOperationManager
//         let deferred_operation_component = engine.get_global_component_mut::<DeferredOperationComponent>().expect("Critical: No DeferredOperationComponent");
//         self.deferred_operation_manager = Some(deferred_operation_component.borrow_deferred_operation_manager());

//         // Check if material handle is valid
//         // if self.material_handle.is_some() {
//         //     engine.get_resource::<Material>(&self.material_handle.unwrap())
//         //         .context(format!("Creating {} {} failed", "Component".general_object_style(), get_type_name::<Self>().specific_object_style()))?;
//         // }

//         // // Check if mesh handle is valid
//         // if self.mesh_handle.is_some() {
//         //     engine.get_resource::<Mesh>(&self.mesh_handle.unwrap())
//         //         .context(format!("Creating {} {} failed", "Component".general_object_style(), get_type_name::<Self>().specific_object_style()))?;
//         // }

//         // Update mesh rendering queue
//        // self.update_render_queue_key(&engine.resource_manager)?;
// // for effect in self.effects.iter_as_trait::<dyn PostprocessingEffect>() {
// //     println!("Effect: {}", effect.name());
// // }
//         // // Create shader for each postprocessing effect
//         // for effect in self.effects.iter_as::<dyn PostprocessingEffect>() {
//         //     println!("Effect: {}", effect.name());
//         // }


//         //let storage: = self.effects.get::<dyn PostprocessingEffect>().unwrap().data.as_ref();

//         // for (_type_id, boxed) in &storage.0 {
//         //     if let Some(effect) = boxed.downcast_ref::<Box<dyn PostprocessingEffect>>() {
//         //         println!("Effect: {}", effect.name());
//         //     }
//         // }


//         Ok(())
//     }

//     fn pass_handles(&mut self, self_scene_handle: SceneHandle, self_entity_handle: EntityHandle) {
//         self.scene_handle = Some(self_scene_handle);
//         self.entity_handle = Some(self_entity_handle);
//     }

//     fn deferred_operation(&mut self, engine: &mut Engine, request: usize) -> Result<()> { 
//         // match request {
//         //     DEFERRED_REQUEST_VARIANT_SET_MATERIAL => 
//         //     {
//         //         // Check if material handle is valid
//         //         engine.get_resource::<Material>(&self.material_handle.unwrap())
//         //             .context(format!("Setting {} {} failed", "Resource".general_object_style(), "Material".specific_object_style()))?;
                
//         //         self.update_render_queue_key(&engine.resource_manager)?;
//         //     },
//         //     DEFERRED_REQUEST_VARIANT_SET_MESH =>
//         //     {
//         //         // Check if mesh handle is valid
//         //         engine.get_resource::<Mesh>(&self.mesh_handle.unwrap())
//         //             .context(format!("Setting {} {} failed", "Resource".general_object_style(), "Mesh".specific_object_style()))?;

//         //         self.update_render_queue_key(&engine.resource_manager)?;
//         //     },
//         //     DEFERRED_REQUEST_VARIANT_UPDATE_RENDER_QUEUE => 
//         //     {
//         //         // Update mesh rendering queue
//         //         self.update_render_queue_key(&engine.resource_manager)?;
//         //     },
//         //     _ => 
//         //     {
//         //         panic!("Critical: Processing deferred update request with value {} in {} failed. Handling is not implemented", request, get_type_name::<Self>().specific_object_style());
//         //     }
//         // }

//         Ok(()) 
//     }
}

