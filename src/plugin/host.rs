use anyhow::{Context, Result};
use std::path::Path;
use wasmtime::{Engine, Linker, Module, Store};
use wasmtime_wasi::preview1::WasiP1Ctx;
use wasmtime_wasi::WasiCtxBuilder;

/// Host state available to WASM plugins.
pub struct PluginHostState {
    pub wasi: WasiP1Ctx,
    pub plugin_name: String,
}

/// Create a wasmtime engine with sensible defaults.
pub fn create_engine() -> Result<Engine> {
    let mut config = wasmtime::Config::new();
    config.wasm_component_model(true);
    config.async_support(false); // Plugins run synchronously for simplicity.
    Engine::new(&config).context("failed to create wasmtime engine")
}

/// Load and instantiate a WASM plugin module.
pub fn load_plugin(
    engine: &Engine,
    wasm_path: &Path,
    plugin_name: &str,
) -> Result<(Store<PluginHostState>, wasmtime::Instance)> {
    let module = Module::from_file(engine, wasm_path)
        .with_context(|| format!("failed to load WASM module: {}", wasm_path.display()))?;

    let mut linker: Linker<PluginHostState> = Linker::new(engine);

    // Link WASI imports for basic I/O
    wasmtime_wasi::preview1::add_to_linker_sync(&mut linker, |state: &mut PluginHostState| {
        &mut state.wasi
    })?;

    // Register host functions that plugins can call
    register_host_functions(&mut linker)?;

    let wasi = WasiCtxBuilder::new()
        .inherit_stderr()
        .build_p1();

    let mut store = Store::new(
        engine,
        PluginHostState {
            wasi,
            plugin_name: plugin_name.to_string(),
        },
    );

    let instance = linker
        .instantiate(&mut store, &module)
        .context("failed to instantiate WASM plugin")?;

    Ok((store, instance))
}

/// Register host functions in the `recalld` namespace that plugins can import.
fn register_host_functions(linker: &mut Linker<PluginHostState>) -> Result<()> {
    // recalld::log(message_ptr, message_len)
    linker.func_wrap(
        "recalld",
        "log",
        |mut caller: wasmtime::Caller<'_, PluginHostState>, ptr: i32, len: i32| {
            let memory = caller.get_export("memory").unwrap().into_memory().unwrap();
            let data = memory.data(&caller);
            let start = ptr as usize;
            let end = start + len as usize;
            if end <= data.len() {
                if let Ok(msg) = std::str::from_utf8(&data[start..end]) {
                    let name = caller.data().plugin_name.clone();
                    tracing::info!(plugin = %name, "{msg}");
                }
            }
        },
    )?;

    Ok(())
}
