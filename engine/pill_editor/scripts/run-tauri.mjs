// Sets CARGO_TARGET_DIR to a platform-specific subdirectory
// so that Windows and Linux build artifacts never collide
// when sharing the project folder across machines.
import { execSync } from "node:child_process";

const platformMap = { win32: "windows", linux: "linux", darwin: "macos" };
const platform = platformMap[process.platform] || process.platform;

const env = { ...process.env, CARGO_TARGET_DIR: `target/${platform}` };

const args = process.argv.slice(2).join(" ");
execSync(`tauri ${args}`, { stdio: "inherit", env });
