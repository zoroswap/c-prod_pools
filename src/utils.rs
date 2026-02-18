use miden_protocol::account::StorageSlotName;

use std::sync::Arc;

use miden_client::assembly::{
    Assembler, DefaultSourceManager, Module, ModuleKind, Path as AssemblyPath,
};

pub fn slot_name(name: &str) -> StorageSlotName {
    StorageSlotName::new(name).expect("valid slot name")
}

pub fn create_library(
    assembler: Assembler,
    library_path: &str,
    source_code: &str,
) -> Result<miden_client::assembly::Library, Box<dyn std::error::Error>> {
    let source_manager = Arc::new(DefaultSourceManager::default());
    println!("parsing library: {:?}", library_path);
    let module = Module::parser(ModuleKind::Library).parse_str(
        AssemblyPath::new(library_path),
        source_code,
        source_manager.clone(),
    )?;
    println!("Module: {:?}", module);
    let library = assembler.clone().assemble_library([module])?;
    Ok(library)
}
