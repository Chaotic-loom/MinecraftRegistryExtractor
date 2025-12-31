use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use walkdir::WalkDir;

// --- Configuration ---
const SERVER_JAR: &str = "./server.jar";
const WORK_DIR: &str = "./temp_data";
const OUTPUT_DIR: &str = "./registries";

#[derive(Deserialize)]
struct TagFile {
    values: Vec<String>,
}

fn main() -> Result<()> {
    println!("--- Minecraft Registry & Tag Builder ---");

    // 1. Generate Data using Java
    if !Path::new(SERVER_JAR).exists() {
        bail!("{} not found!", SERVER_JAR);
    }
    generate_data()?;

    let data_path = Path::new(WORK_DIR).join("generated/data/minecraft");
    if !data_path.exists() {
        bail!("Data generation failed.");
    }

    // 2. Prepare Output
    if Path::new(OUTPUT_DIR).exists() {
        fs::remove_dir_all(OUTPUT_DIR)?;
    }
    fs::create_dir_all(OUTPUT_DIR)?;

    // 3. Process Registries
    // We need to keep a mapping of "Entry Name" -> "Integer ID" for the Tags step
    // Map<RegistryID, Map<EntryID, ProtocolID>>
    let mut registry_mappings: HashMap<String, HashMap<String, i32>> = HashMap::new();

    println!("Processing Registries...");
    compile_registries(&data_path, &mut registry_mappings)?;

    // 4. Process Tags
    println!("Processing Tags...");
    compile_tags(&data_path.join("tags"), &registry_mappings)?;

    println!("--- Success! Output in {} ---", OUTPUT_DIR);
    Ok(())
}

fn generate_data() -> Result<()> {
    if Path::new(WORK_DIR).exists() { fs::remove_dir_all(WORK_DIR)?; }
    fs::create_dir_all(WORK_DIR)?;

    let jar_abs = fs::canonicalize(SERVER_JAR)?;

    println!("Running Java data generator...");
    let status = Command::new("java")
        .current_dir(WORK_DIR)
        .arg("-DbundlerMainClass=net.minecraft.data.Main")
        .arg("-jar").arg(jar_abs).arg("--all")
        .output()?;

    if !status.status.success() {
        bail!("Java failed: {}", String::from_utf8_lossy(&status.stderr));
    }
    Ok(())
}

fn compile_registries(base_path: &Path, mappings: &mut HashMap<String, HashMap<String, i32>>) -> Result<()> {
    // 1. Find all registries and their entries
    let mut registries: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();

    for entry in WalkDir::new(base_path).min_depth(1).into_iter().filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.extension().map_or(false, |e| e == "json") {
            // Check if this file is inside a "tags" folder, if so, skip it (handled later)
            if path.components().any(|c| c.as_os_str() == "tags") {
                continue;
            }

            let relative = path.strip_prefix(base_path)?;
            if let Some(parent) = relative.parent() {
                let registry_name = parent.to_string_lossy().replace("\\", "/");
                let entry_name = path.file_stem().unwrap().to_string_lossy().to_string();

                let full_reg = format!("minecraft:{}", registry_name);
                let full_entry = format!("minecraft:{}", entry_name);

                registries.entry(full_reg).or_default().insert(full_entry);
            }
        }
    }

    // 2. Write Registry Packets (0x07)
    for (reg_id, entries) in registries {
        let mut buffer: Vec<u8> = Vec::new();

        // Header
        write_string(&mut buffer, &reg_id)?;
        write_varint(&mut buffer, entries.len() as i32)?;

        // Entries
        let mut id_map = HashMap::new();
        for (idx, entry_id) in entries.iter().enumerate() {
            // Write to packet
            write_string(&mut buffer, &entry_id)?;
            buffer.push(0x00); // Has Data? -> False

            // Store ID for mapping (Tags need this ID)
            id_map.insert(entry_id.clone(), idx as i32);
        }

        mappings.insert(reg_id.clone(), id_map);

        // Save file
        let filename = format!("{}.bin", reg_id.replace(":", "_").replace("/", "_"));
        fs::write(Path::new(OUTPUT_DIR).join(filename), &buffer)?;
    }
    Ok(())
}

fn compile_tags(tags_path: &Path, registry_mappings: &HashMap<String, HashMap<String, i32>>) -> Result<()> {
    // Map<RegistryID, Map<TagID, Vec<Integers>>>
    let mut tags_packet_data: BTreeMap<String, BTreeMap<String, Vec<i32>>> = BTreeMap::new();

    for entry in WalkDir::new(tags_path).into_iter().filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.extension().map_or(false, |e| e == "json") {
            let relative = path.strip_prefix(tags_path)?;

            // Format: <registry>/<tag>.json OR <registry>/<sub_path>/<tag>.json
            if let Some(parent) = relative.parent() {
                let registry_suffix = parent.components().next().unwrap().as_os_str().to_string_lossy();
                let full_reg = format!("minecraft:{}", registry_suffix);

                // If we don't have this registry in our mappings, we can't build tags for it.
                if !registry_mappings.contains_key(&full_reg) { continue; }

                // Tag name is everything after the registry folder
                let tag_suffix = relative.strip_prefix(&*registry_suffix)?.with_extension("").to_string_lossy().replace("\\", "/");
                let full_tag = format!("minecraft:{}", tag_suffix);

                // Parse JSON
                let content = fs::read_to_string(path)?;
                let parsed: TagFile = serde_json::from_str(&content).unwrap_or(TagFile { values: vec![] });

                let mut ids = Vec::new();
                for value in parsed.values {
                    // Simple resolution: direct reference only.
                    // (Vanilla tags sometimes use # for nested tags,
                    // this simple parser skips them to prevent complexity,
                    // which is usually fine for the "required" registry tags)
                    if !value.starts_with("#") {
                        if let Some(id) = registry_mappings.get(&full_reg).and_then(|m| m.get(&value)) {
                            ids.push(*id);
                        }
                    }
                }

                tags_packet_data.entry(full_reg).or_default().insert(full_tag, ids);
            }
        }
    }

    // Write "packet_tags.bin" (Packet ID 0x0D body)
    let mut buffer: Vec<u8> = Vec::new();

    // Registry Count
    write_varint(&mut buffer, tags_packet_data.len() as i32)?;

    for (reg_name, tags) in tags_packet_data {
        write_string(&mut buffer, &reg_name)?; // Registry Name
        write_varint(&mut buffer, tags.len() as i32)?; // Tag Count

        for (tag_name, ids) in tags {
            write_string(&mut buffer, &tag_name)?; // Tag Name
            write_varint(&mut buffer, ids.len() as i32)?; // ID Count
            for id in ids {
                write_varint(&mut buffer, id)?; // ID
            }
        }
    }

    fs::write(Path::new(OUTPUT_DIR).join("packet_tags.bin"), &buffer)?;
    Ok(())
}

fn write_varint(buf: &mut Vec<u8>, mut value: i32) -> Result<()> {
    loop {
        let mut temp = (value & 0x7F) as u8;
        value >>= 7;
        if value != 0 { temp |= 0x80; }
        buf.push(temp);
        if value == 0 { break; }
    }
    Ok(())
}

fn write_string(buf: &mut Vec<u8>, s: &str) -> Result<()> {
    let bytes = s.as_bytes();
    write_varint(buf, bytes.len() as i32)?;
    buf.extend_from_slice(bytes);
    Ok(())
}