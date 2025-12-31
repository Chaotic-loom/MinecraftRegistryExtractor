use anyhow::{Context, Result, bail};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use walkdir::WalkDir;

// --- Configuration ---
const SERVER_JAR: &str = "./server.jar";
const WORK_DIR: &str = "./temp_data"; // Where we run the java command
const OUTPUT_DIR: &str = "./registries"; // Where we save the ready-to-send packets

fn main() -> Result<()> {
    println!("--- Minecraft Registry Builder ---");

    // 1. Verify Environment
    if !Path::new(SERVER_JAR).exists() {
        bail!("Could not find '{}'. Please place your 1.21.x server jar in the root directory.", SERVER_JAR);
    }

    // Check for Java
    let java_check = Command::new("java").arg("-version").output();
    if java_check.is_err() {
        bail!("Java is not installed or not in your PATH.");
    }

    // 2. Run Data Generator (Replaces extract.sh)
    println!("Step 1: Generating data from server.jar...");
    generate_data()?;

    // 3. Process and Compile
    println!("Step 2: compiling registry packets...");
    // The data generator outputs to <WORK_DIR>/generated/data/minecraft
    let input_path = Path::new(WORK_DIR).join("generated/data/minecraft");

    if !input_path.exists() {
        bail!("Data generation failed. Could not find {:?}", input_path);
    }

    compile_registries(&input_path, Path::new(OUTPUT_DIR))?;

    println!("--- Success! ---");
    println!("Registry packets are ready in '{}'", OUTPUT_DIR);

    Ok(())
}

fn generate_data() -> Result<()> {
    // Clean/Create working directory
    if Path::new(WORK_DIR).exists() {
        fs::remove_dir_all(WORK_DIR).context("Failed to clean temp directory")?;
    }
    fs::create_dir_all(WORK_DIR).context("Failed to create temp directory")?;

    // We need the absolute path to the jar because we are changing the current dir
    let jar_abs_path = fs::canonicalize(SERVER_JAR).context("Failed to get absolute path of server.jar")?;

    println!("   Running Java data generator... (this may take a moment)");

    // Command: java -DbundlerMainClass="net.minecraft.data.Main" -jar server.jar --all
    let status = Command::new("java")
        .current_dir(WORK_DIR) // Run INSIDE temp_data so 'generated' folder appears there
        .arg("-DbundlerMainClass=net.minecraft.data.Main")
        .arg("-jar")
        .arg(jar_abs_path)
        .arg("--all")
        .status()
        .context("Failed to execute Java command")?;

    if !status.success() {
        bail!("Java process exited with error code: {:?}", status.code());
    }

    Ok(())
}

fn compile_registries(input_path: &Path, output_path: &Path) -> Result<()> {
    // Map<RegistryName, List<EntryName>>
    let mut registries: BTreeMap<String, Vec<String>> = BTreeMap::new();

    // Scan for JSON files
    for entry in WalkDir::new(input_path).min_depth(1).into_iter().filter_map(|e| e.ok()) {
        let path = entry.path();

        if path.extension().and_then(|s| s.to_str()) == Some("json") {
            // Path structure is: .../data/minecraft/<registry_name>/<entry_name>.json
            // We want to grab <registry_name> and <entry_name>

            // We need to be careful with paths.
            // The `generated/data/minecraft` folder contains folders like `damage_type`, `worldgen`, etc.
            // Some registries are nested, like `worldgen/biome`.

            let relative_path = path.strip_prefix(input_path)?;

            if let Some(parent) = relative_path.parent() {
                // If the file is 'damage_type/fall.json', parent is 'damage_type'
                // If the file is 'worldgen/biome/plains.json', parent is 'worldgen/biome'

                // Convert path separator to standard registry format (slashes)
                let registry_suffix = parent.to_string_lossy().replace("\\", "/");
                let entry_name = path.file_stem().unwrap().to_string_lossy().to_string();

                let full_registry_id = format!("minecraft:{}", registry_suffix);
                let full_entry_id = format!("minecraft:{}", entry_name);

                registries
                    .entry(full_registry_id)
                    .or_default()
                    .push(full_entry_id);
            }
        }
    }

    // Prepare Output
    if output_path.exists() {
        fs::remove_dir_all(output_path)?;
    }
    fs::create_dir_all(output_path)?;

    // Serialize
    for (reg_id, mut entries) in registries {
        // Sort for deterministic output
        entries.sort();

        let mut buffer: Vec<u8> = Vec::new();

        // 1. Registry Identifier
        write_string(&mut buffer, &reg_id)?;

        // 2. Entry Count
        write_varint(&mut buffer, entries.len() as i32)?;

        // 3. Entries
        for entry_id in entries {
            write_string(&mut buffer, &entry_id)?;
            // Has Data? -> False (0x00)
            buffer.push(0x00);
        }

        // Filename safe: minecraft:worldgen/biome -> minecraft_worldgen_biome.bin
        let safe_name = reg_id.replace(":", "_").replace("/", "_");
        let filename = format!("{}.bin", safe_name);

        let mut file = File::create(output_path.join(filename))?;
        file.write_all(&buffer)?;
    }

    Ok(())
}

// --- Binary Helpers ---

fn write_varint(buf: &mut Vec<u8>, mut value: i32) -> Result<()> {
    loop {
        let mut temp = (value & 0x7F) as u8;
        value >>= 7;
        if value != 0 {
            temp |= 0x80;
        }
        buf.push(temp);
        if value == 0 {
            break;
        }
    }
    Ok(())
}

fn write_string(buf: &mut Vec<u8>, s: &str) -> Result<()> {
    let bytes = s.as_bytes();
    write_varint(buf, bytes.len() as i32)?;
    buf.extend_from_slice(bytes);
    Ok(())
}