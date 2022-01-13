use std::{io::{BufReader, BufRead}, fs::File, path::PathBuf};

extern crate winres;

fn main() {
    // Set icon of executable
    if cfg!(target_os = "windows") {
        let mut windows_resource = winres::WindowsResource::new();
        
        // Get path to game project from cargo.toml of standalone
        let mut icon_path = "".to_string();
        let input_file = File::open("./Cargo.toml").unwrap();
        let lines = BufReader::new(input_file).lines().map(|v| v.unwrap()).collect::<Vec<String>>();
        for line in lines {
            if line.contains("pill_game") {
                let start_index = line.find("\"").expect("pill_game dependency in pill_standalone manifest is invalid");
                let end_index = line.rfind("\"").expect("pill_game dependency in pill_standalone manifest is invalid");
                icon_path = line.to_string()[start_index + 1..end_index].to_string();
                break;
            }
        }

        if icon_path.is_empty() {
            panic!("Cannot find icon for executable in game res directory");
        }

        let icon_path = PathBuf::from(icon_path).join("res").join("icon.ico");

        if icon_path.exists() {
            windows_resource.set_icon(icon_path.to_str().unwrap());          
        }  
        else {
            windows_resource.set_icon("./res/icon.ico");
        }
        windows_resource.compile().unwrap();
    }
}