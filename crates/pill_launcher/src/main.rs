extern crate clap;
extern crate fs_extra;

use std::process::Command;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use fs_extra::{copy_items, file};
use fs_extra::dir::CopyOptions;

use clap::{Arg, App};

fn convert_all_characters(word: String, from: char, to: char) -> String {
    word
}

fn copy_all_built_files(from_dir: &String, out_dir: &String) {
    let mut copy_options = CopyOptions::new();
    copy_options.overwrite = true;
    let mut paths_to_copy = Vec::new();
    paths_to_copy.push(from_dir);
    copy_items(&paths_to_copy, out_dir, &copy_options);
}

fn copy_exe(from_dir: &String, out_dir: &String, file_name: &String) {
    fs::create_dir_all(out_dir.to_owned() + "/target/debug");
    let mut copy_options = CopyOptions::new();
    copy_options.overwrite = true;
    let mut paths_to_copy = Vec::new();
    paths_to_copy.push(from_dir.to_owned() + "/" + file_name + ".exe");
    copy_items(&paths_to_copy, out_dir.to_owned() + "/target/debug", &copy_options);
}

fn create_game_config(path: &String, game_name: &String) {
    let file = File::create(path.to_owned() + "/game.config").unwrap();
    writeln!(&file, "GAME_NAME={}", &game_name);
}

fn extract_game_name(path: &String) -> &str {
    let mut split = path.split("/").collect::<Vec<&str>>();
    let last_word = split.last().unwrap().clone();
    println!("{}", last_word);
    return last_word;
}

fn make_cargo_new(path: &String) {
    Command::new("cmd")
        .args(&["/C", "cargo", "new", path])
        .status()
        .expect("Failed to execute command");
}

fn make_cargo_build(path: &String) {
    let cargo_path = path.to_owned() + "\\Cargo.toml";
    println!("{}", cargo_path);
    Command::new("cmd")
        .args(&["/C", "cargo", "build", "--manifest-path", &cargo_path])
        .status()
        .expect("Failed to execute command");
}

fn make_cargo_run(path: &String) {
    let cargo_path = path.to_owned() + "\\Cargo.toml";
    Command::new("cmd")
        .args(&["/C", "cargo", "run", "--manifest-path", &cargo_path])
        .status()
        .expect("Failed to execute command");
}

fn create_game_file(path: &String) {
    let game_file_path = path.to_owned() + "/src/game.rs";
    let template_path = "../templates/game_template.txt";
    
    let template_file = File::open(template_path).unwrap();
    let mut game_file = File::create(game_file_path).unwrap();
    let template_lines = BufReader::new(template_file).lines()
        .map(|x| x.unwrap()).collect::<Vec<String>>();

    for line in template_lines {
        writeln!(game_file, "{}", &line);
    }
}

fn create_lib_file(path: &String) {
    let lib_file_path = path.to_owned() + "/src/lib.rs";
    let template_path = "../templates/lib_template.txt";
    
    let template_file = File::open(template_path).unwrap();
    let mut lib_file = File::create(lib_file_path).unwrap();
    let template_lines = BufReader::new(template_file).lines()
        .map(|x| x.unwrap()).collect::<Vec<String>>();

    for line in template_lines {
        writeln!(lib_file, "{}", &line);
    }
}

fn overwrite_cargo_toml(path: &String) {
    let cargo_path = path.to_owned() + "/cargo.toml";

    let _cargo_file = match File::create(cargo_path) {
        Err(err) => panic!("Couldn't open cargo.toml in new game's directory: {}", err),
        Ok(file) => {
            let template_path = "../templates/game_cargo_template.txt";
            let template_file = File::open(template_path).unwrap();
            let template_lines = BufReader::new(template_file).lines()
                .map(|x| x.unwrap()).collect::<Vec<String>>();
            for line in template_lines {
                writeln!(&file, "{}", &line);
            }
        }
    };
}

fn set_dependencies(path: &String, game_name: &String) {
    let file = File::open("../../pill_standalone/Cargo.toml").unwrap();
    let lines = BufReader::new(file).lines().map(|x| x.unwrap()).collect::<Vec<String>>();

    let new_cargo = File::create("../../pill_standalone/Cargo.toml").unwrap();
    for line in lines {
        if line.contains("pill-game") {
            writeln!(&new_cargo, "pill-game = {{path = \"{}\"}} # This needs to be pointing to the correct game directory", &path);
        }
        else if line.contains("name = ") {
            writeln!(&new_cargo, "name = \"{}\"", &game_name);
        }
        else {
            writeln!(&new_cargo, "{}", &line);
        }
    }
}

fn main() {
    let app = App::new("Pill Engine Game Launcher")
        .about("CLI for launching or creating game project for Pill Engine")
        .author("Mateusz Szymoński & Łukasz Zalewski");

    // Definition of the options for the CLI
    let action_option = Arg::with_name("a")
        .long("action") // allow --name
        .takes_value(true)
        .help("Main option defining the action, which we are taking")
        .required(true);

    let name_option = Arg::with_name("o")
        .long("name")
        .takes_value(true)
        .help("Name for the game project")
        .default_value("NewGame")
        .required(false);

    let path_option = Arg::with_name("p")
        .long("path")
        .takes_value(true)
        .help("Path to the game directory")
        .required(true);

    // Addition of the options to the CLI
    let app = app.arg(action_option).arg(name_option).arg(path_option);

    // Extraction of the arguments
    let matches = app.get_matches();

    let action = matches.value_of("a")
        .expect("Action has to be defined");

    let path = matches.value_of("p")
        .expect("Path has to be defined");

    let name = matches.value_of("o")
        .expect("Name of the game has to be defined");

    match action {
        "create_game_project" => {
            let game_path = path.to_owned() + "/" + &name;

            println!("Creating game directory...");
            make_cargo_new(&game_path);

            println!("Creating game file...");
            create_game_file(&game_path);

            println!("Creating lib file...");
            create_lib_file(&game_path);

            println!("Creating config file...");
            create_game_config(&game_path, &String::from(name));

            println!("Overwriting cargo.toml...");
            overwrite_cargo_toml(&game_path);

            println!("Done!");
        },
        "build_game" => {
            let game_path = path.to_owned();
            let standalone_path = "..\\..\\pill_standalone";
            let game_name = extract_game_name(&game_path);
            
            println!("Setting dependencies in standalone to game...");
            set_dependencies(&game_path, &String::from(game_name));

            println!("Building game...");
            make_cargo_build(&String::from(standalone_path));

            println!("Copying built exe file...");
            copy_exe(&String::from("../../pill_standalone/target/debug/".to_owned()), &String::from("../../../".to_owned() + game_name), &String::from(game_name));
            //fs::remove_dir_all(&String::from("../../pill_standalone/target/".to_owned()));

            println!("Done!");
        }   
        _ => { 
            println!("Undefined action");
        }
    };
}
