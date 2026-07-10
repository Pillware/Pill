use std::path::Path;

use pill_core::{ErrorContext, Result};

use crate::serdeserialization::serdeser::SerializedScene;

pub trait SerdeserBackend {
    fn write_scene(&mut self, file: &Path, scene: &SerializedScene) -> Result<()>;
    fn read_scene(&mut self, file: &Path) -> Result<SerializedScene>;
}

pub struct JsonBackend;

impl SerdeserBackend for JsonBackend {
    fn write_scene(&mut self, file: &Path, scene: &SerializedScene) -> Result<()> {
        let writer = std::fs::File::create(file)?;
        serde_json::to_writer_pretty(writer, &scene)?;
        Ok(())
    }

    fn read_scene(&mut self, file: &Path) -> Result<SerializedScene> {
        let reader = std::fs::File::open(file)?;
        let scene = serde_json::from_reader(reader)?;
        Ok(scene)
    }
}
