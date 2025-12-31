# Minecraft Registry Extractor
A Rust utility for custom Minecraft servers (1.21+). It automates the extraction and compilation of Registry Data Packets required during the protocol's Configuration Phase.

Instead of hardcoding thousands of registry entries (Biomes, Damage Types, Dimensions) or writing complex NBT logic at runtime, this tool "bakes" the vanilla server data into raw binary files that you can send directly to the client.

This is going to be used for [Nullspace, a custom Minecraft server made on Rust](https://github.com/Chaotic-loom/Nullspace)