<p align="left">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="media/logo/pill_logo_horizontal_white.png">
    <img src="media/logo/pill_logo_horizontal_white.png">
  </picture>
</p>

Data-driven game engine written in Rust

## Design Goals
- Clean and simple
- Blazing fast
- Highly extensible

## Features
- Archetype-based Entity Component System 
- 3D graphics (Blinn-Phong shading model, instancing)
- Scenes
- Input handling (keyboard, mouse, gamepad)
- Sound playing (mono, spatial)
- Resource system (mesh, texture, shader, material, sound)
- Material system and custom shader loading
- Custom systems, components and resources support
- Error chaining
- Launcher tool
- Game project hot-reloading
- Shader code hot-reloading 🚧
- Lights 🚧
- Skybox 🚧
- Post-processing 🚧
- Networking 🚧
- Physics 🚧
- Configurable logging 🚧
- Editor 🚧

## Getting Started
1. Install Rust  
https://www.rust-lang.org/tools/install
2. Download and unpack this repository
3. Build Pill Launcher  
`cargo build --release --manifest-path <ENGINE_PATH>\Pill-Engine\engine\pill_launcher\Cargo.toml`
4. Add Pill Launcher to PATH (optional)  
On Windows: follow [these steps](https://superuser.com/questions/1861276/how-to-set-a-folder-to-the-path-environment-variable-in-windows-11) add `<ENGINE_PATH>\Pill-Engine\engine\pill_launcher\target\release`
On Linux: `echo 'export PATH="$PATH:<ENGINE_PATH>/Pill-Engine/engine/pill_launcher/target/release"' >> ~/.bashrc && source ~/.bashrc`  
and restart terminal
5. Create new game project  
`PillLauncher.exe -a create -n Hello-Pill`
6. Run it!  
`PillLauncher.exe -a run -p ./Hello-Pill`

Check [demo](examples/Floating-Pills "demo")!

## Documentation
[Repository](https://github.com/MattSzymonski/Pill-Engine-Docs "Repository")

- For game developers - [Docs](https://raw.githack.com/MattSzymonski/Pill-Engine-Docs/main/docs/game_dev/doc/pill_engine/game/index.html "Docs")  
- For engine developers - [Docs](https://raw.githack.com/MattSzymonski/Pill-Engine-Docs/main/docs/engine_dev/doc/pill_engine/index.html "Docs")  

## Showcase
<p align="center">
  <img src="examples/Floating-Pills/media/floating_pills_1.gif" img width=100%>
</p>

<p align="center">
  <img src="media/logo/pill_pile.png" img width=100%>
</p>