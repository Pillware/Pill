mod project;
use pill_engine::project::create_project;

create_project!(
    crate::project::Project {},
    pill_engine::project::PillProject
);
