(Floating Pills, or any other assuming that it is in examples folder)

Compilation has to be done together on pill_standalone and pill_game together in the same context. 
For that compilation through Cargo workspace is required.
Otherwise, typeids of types like "Mesh" will not match what will make all generic (templated) functions work improperly

### Build pill launcher:
`cargo build --manifest-path engine/pill_launcher/Cargo.toml --release`

### Build and run game:
`D:\Programming\Pill-Engine\examples\Floating-Pills>PillLauncher.exe -a run -p . `

PillLauncher will change workspace path in game's Cargo.toml to path to the engine workspace folder:
```
[package]
name = "pill_game"
version = "0.1.0"
edition = "2021"
workspace = "D:/Programming/Pill-Engine/engine"

...
```

Also it will adjust path in the workspace cargo.toml at: `D:\Programming\Pill-Engine\engine\Cargo.toml`
So it points to game folder:
```
members = [
    "pill_core", 
    "pill_engine", 
    "pill_renderer", 
    "pill_standalone",
    "D:/Programming/Pill-Engine/examples/Floating-Pills", ### Game project crate (This will be changed by Pill Launcher on build to allow proper compilation of game project)
]
```

PillLauncher will:
Output standalone.exe to:
`D:\Programming\Pill-Engine\examples\Floating-Pills\build\dev\Floating-Pills.exe`

Output game DLL to:
`D:\Programming\Pill-Engine\examples\Floating-Pills\build\dev\data\pill-game.dll`

### HotReloading

Running standalone.exe (`Floating-Pills.exe`) will detect change in the src folder and will call PillLauncher that will output hot reloaded DLL to:
`D:\Programming\Pill-Engine\examples\Floating-Pills\build\dev\data\pill-game-hot-reloaded.dll`
Then it will stop engine, unload current `pill-game.dll`, delete it, rename `pill-game-hot-reloaded.dll` to pill-game.dll, load it as new and start the engine
