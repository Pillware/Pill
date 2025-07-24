#![cfg(feature = "rendering")]

use crate::{
    engine::Engine,
    graphics::{ RendererTextureHandle },
    resources::{ ResourceStorage, Resource, ResourceLoadType, Material },
    ecs::{ DeferredUpdateManagerPointer, AudioSourceComponent, SoundType, AudioManagerComponent },
    config::*,
};

use pill_core::{ PillSlotMapKey, PillTypeMapKey, PillStyle, get_type_name, EngineError };

use std::{
    collections::HashSet,
    io::{ BufRead, Read, Cursor},
    path::{ Path, PathBuf },
    fs::File,
};
use anyhow::{ Result, Context, Error };
use rodio::{ Source, source::Buffered, Decoder };


pill_core::define_new_pill_slotmap_key! {
    pub struct SoundHandle;
}

#[readonly::make]
pub struct Sound {
    #[readonly]
    pub name: String,
    #[readonly]
    pub path: PathBuf,
    pub(crate) sound_data: Option<SoundData>
}

impl Sound {
    pub fn new(name: &str, path: PathBuf) -> Self {
        Self {
            name: name.to_string(),
            path,
            sound_data: None
        }
    }
}

impl PillTypeMapKey for Sound {
    type Storage = ResourceStorage<Sound>;
}

impl Resource for Sound {
    type Handle = SoundHandle;

    fn initialize(&mut self, engine: &mut Engine) -> Result<()> {
        let error_message = format!("Initializing {} {} failed", "Resource".gobj_style(), get_type_name::<Self>().sobj_style());

        // Check if path to asset is correct
        pill_core::validate_asset_path(&self.path, &["mp3", "wav"]).context(error_message.clone())?;

        // Create sound data
        let sound_data = SoundData::new(&self.path).context(error_message.clone())?;
        self.sound_data = Some(sound_data);

        Ok(())
    }

    fn get_name(&self) -> String {
        self.name.clone()
    }

    fn destroy<H: PillSlotMapKey>(&mut self, engine: &mut Engine, self_handle: H) -> Result<()> {
        // Find audio source components that use this sound and update them
        for (scene_handle, scene) in engine.scene_manager.scenes.iter_mut() {
            for (entity_handle, audio_source_component) in scene.get_one_component_iterator_mut::<AudioSourceComponent>()? {
                if let Some(sound_handle) = audio_source_component.sound_handle {
                    // If audio source component has handle to this sound
                    if sound_handle.data() == self_handle.data() {
                        audio_source_component.remove_sound();
                    }
                }
            }
        }

        Ok(())
    }
}


pub struct SoundData {
    pub(crate) source_buffer: Vec<u8>
}

impl SoundData {
    pub fn new(path: &PathBuf) -> Result<Self> {
        // Open sound file
        let mut sound_file = match File::open(path) {
            Err(err) => return Err(Error::new(EngineError::InvalidAssetPath(path.clone().into_os_string().into_string().unwrap()))),
            file => file?
        };

        // Read bytes to vector
        let mut sound_data = Vec::new();
        sound_file.read_to_end(&mut sound_data).unwrap();

        // Create SoundData
        let sound_data = SoundData {
            source_buffer: sound_data
        };

        Ok(sound_data)
    }

    pub fn get_source_sound(&self) -> Decoder<Cursor<Vec<u8>>> {
        let mut sound_source = Vec::<u8>::new();

        // Read bytes from the buffer
        for buffer in self.source_buffer.iter() {
            sound_source.push(buffer.clone());
        }

        // Return decoded bytes as the sound, which can be played
        Decoder::new(Cursor::new(sound_source)).unwrap()
    }
}
