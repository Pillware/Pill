use pill_engine::project::*;

pub struct Project {}
create_project!(Project {}, PillProject);

impl PillProject for Project {
    fn start(&self, engine: &mut Engine) -> Result<()> {
        Ok(())
    }
}
